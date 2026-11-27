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

#[path = "wait_diag.rs"]
pub(crate) mod wait_diag;
#[path = "submission_safety.rs"]
pub(crate) mod submission_safety;
#[path = "chain_trace.rs"]
pub(crate) mod chain_trace;

/// One submission being recorded: its two command buffers, staging memory, descriptor pools and what its completion
/// must do.
pub struct Sub {
    pub pre: vk::CommandBuffer,
    pub main: vk::CommandBuffer,
    pub stage: Vec<Buf>,
    stage_used: u64,
    pub dpools: submission_safety::DescriptorPools<vk::DescriptorPool>,
    pub dpool_i: usize,
    /// (pool base, guest address, bytes, pool buffer, buffer offset, pool identity)
    pub writebacks: Vec<(u64, u64, u64, vk::Buffer, u64, u64)>,
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
    /// texture views by (texture ID, sampler type, storage, registration/backing generation)
    pub memo_views: std::collections::HashMap<(i32, u32, bool, u64), vk::ImageView>,
    /// Immutable pool descriptors last used by this recording: (generation, set, descriptor-buffer address).
    pub pool_snapshot: Option<(u64, vk::DescriptorSet, u64)>,
    /// which recording this is (a stage's staged driver constants are reused by it while they stand)
    pub serial: u64,
    /// which upload buffer `pre` is: this recording's (`serial`), or while a parallel run's segment is translated the
    /// parent's it shares (`share_pre`); what runs once per upload buffer (a GPU-written twin's copy) is keyed by it
    pub pre_serial: u64,
    /// the present count when it was recorded (counter reports: the frame it belongs to)
    pub frame: u64,
    /// raw global memory (`gmem`): the staged memory pool table, the write log and the table's ranges
    pub gmem: Option<super::gmem::Version>,
    /// descriptor buffers: the stage sets written for this submission (chunks), the bytes used of the current one,
    /// and which chunk the main command buffer has bound
    pub db: Vec<super::tables::DescBuf>,
    pub db_cur: usize,
    pub db_used: u64,
    pub db_bound: Option<usize>,
    /// `NXGFX_DB_POISON` (diagnosis): the stage sets written, (chunk, offset, stage)
    pub db_log: Vec<(usize, u64, u32)>,
    /// segments of this submission's stream translated into command buffers of their own (`exec::run_segmented`),
    /// in stream order: submitted after this one's, retired with it
    pub children: Vec<Sub>,
    /// a child's own query pools while it writes its parent's (`child_begin`)
    own_queries: Option<(vk::QueryPool, vk::QueryPool)>,
    /// the translation lane whose command pools its buffers come from (0: the replay thread's, `lane_sub`)
    pub lane: usize,
    /// children translated at the same time take their parent's timestamp and occlusion queries from these counters
    qshare: Option<std::sync::Arc<[std::sync::atomic::AtomicU32; 2]>>,
    /// children whose segment was translated again (`exec::run_segmented`): never submitted, retired with this one
    pub dropped: Vec<Sub>,
}

pub(crate) const DB_CHUNK: u64 = 4 << 20;
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
    Event {
        addr: u64,
        mode: u32,
        value: u32,
    },
    Counter {
        addr: u64,
        kind: u32,
        query: u32,
    },
    /// an occlusion query's samples join the SAMPLES_PASSED sum
    Occl {
        q: u32,
    },
    OcclReset,
    OcclReport {
        addr: u64,
    },
    Readback {
        dst: *mut u8,
        src: *const u8,
        n: usize,
    },
    Callback(Box<dyn FnOnce(&Gpu) + Send>),
    /// run at completion before the write-backs land and before the other posts (raw global memory's write log: what
    /// it names must be known before the game can see this submission complete); gets the submission's GPU timeline value
    Early(Box<dyn FnOnce(&Gpu, u64) + Send>),
}
unsafe impl Send for Post {}

struct Inflight {
    value: u64,
    sub: Sub,
    wb: Vec<(u64, u64, u64, *const u8, *const u8, u64)>,
    readback: Option<Buf>,
    before: Option<Buf>,
    /// the recording epoch its GPU-write notes stamped (`upskip`)
    epoch: u64,
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
        let mut tci = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(0);
        let sci = vk::SemaphoreCreateInfo::default().push_next(&mut tci);
        let timeline =
            unsafe { d.dev.create_semaphore(&sci, None) }.map_err(|e| format!("timeline: {e}"))?;
        let pci = vk::CommandPoolCreateInfo::default()
            .queue_family_index(d.qfam)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let cmd_pool = unsafe { d.dev.create_command_pool(&pci, None) }
            .map_err(|e| format!("command pool: {e}"))?;
        let main_pool = unsafe { d.dev.create_command_pool(&pci, None) }
            .map_err(|e| format!("command pool: {e}"))?;
        Ok(Queue {
            timeline,
            sub: Mutex::new(SubmitState {
                qstates: HashMap::new(),
                parked: HashMap::new(),
                cmd_pool,
                main_pool,
                free_subs: Vec::new(),
            }),
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
        // ops still queued for the replay thread may use it: after them. A translation lane of a parallel run records
        // for the replay thread, inside its current submission: the list below (audit L2; queued as a call it bumped
        // the queue's tail, split submit merging and woke the walker)
        if aq_busy() && !on_replay_thread() && !super::gate::parallel() {
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
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                self.stage[b].ptr.add(off as usize),
                src.len(),
            )
        };
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
            .buffer(
                size,
                vk::BufferUsageFlags::STORAGE_BUFFER
                    | vk::BufferUsageFlags::TRANSFER_SRC
                    | vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuOnly,
                "scratch",
            )
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
        let b = mk(g)
            .or_else(|_| {
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
        // a parallel run's segments share the upload buffer: recorded under its lock only (`gate::pre_lock`)
        if !super::gate::pre_held() {
            crate::log::red_once("an upload recorded into a parallel run's shared upload buffer without its lock (gate::pre_lock)");
        }
        // staged fresh: memoising by content hashed every uploaded byte (OoT: ~56 MB a frame through a serial 128-bit
        // FNV hash and a map insert, for pool pages never repeated within a submission)
        let (sb, so) = self.stage_bytes_new(g, src);
        let r = [vk::BufferCopy {
            src_offset: so,
            dst_offset: off,
            size: src.len() as u64,
        }];
        super::lost::checkpoint(g, self.pre, true, || {
            format!(
                "an upload of {:#x} bytes into buffer {:#x} at {off:#x}",
                src.len(),
                vk::Handle::as_raw(dst)
            )
        });
        unsafe { g.dev().cmd_copy_buffer(self.pre, sb, dst, &r) };
    }
    /// A descriptor set of the given stage set layout, from this submission's pools.
    pub fn alloc_set(&mut self, g: &Gpu, layout: vk::DescriptorSetLayout) -> vk::DescriptorSet {
        let result = submission_safety::allocate_descriptor_set(
            &mut self.dpools.reusable,
            &mut self.dpool_i,
            || {
                let sizes = [
                    vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::UNIFORM_BUFFER,
                        descriptor_count: 32 * 512,
                    },
                    vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::STORAGE_BUFFER,
                        descriptor_count: 32 * 512,
                    },
                    vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 64 * 512,
                    },
                    vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::STORAGE_IMAGE,
                        descriptor_count: 16 * 512,
                    },
                    vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::UNIFORM_TEXEL_BUFFER,
                        descriptor_count: 16 * 512,
                    },
                    vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::STORAGE_TEXEL_BUFFER,
                        descriptor_count: 16 * 512,
                    },
                ];
                let ci = vk::DescriptorPoolCreateInfo::default()
                    .max_sets(512)
                    .pool_sizes(&sizes);
                unsafe { g.dev().create_descriptor_pool(&ci, None) }
            },
            |pool| {
                let l = [layout];
                let ai = vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(*pool)
                    .set_layouts(&l);
                unsafe { g.dev().allocate_descriptor_sets(&ai) }.map(|sets| sets[0])
            },
            |error| submission_safety::descriptor_capacity_error(error.as_raw()),
        );
        // Terminal errors or a fresh pool that cannot fit must not grow resources forever.
        // No cleanup: recorded/inflight commands may still reference the owned pools.
        submission_safety::resource_safety_result(result)
    }
    /// Descriptor buffers: room for `n` bytes of stage sets in the current chunk (a new chunk when it is full; the main
    /// command buffer binds it at its next draw).
    pub fn db_reserve(&mut self, g: &Gpu, n: u64) {
        if self.db.is_empty() || self.db_used + n > self.db[self.db_cur].size {
            if !self.db.is_empty() {
                self.db_cur += 1;
            }
            if self.db_cur == self.db.len() {
                let b = super::tables::desc_buffer(&g.d, DB_CHUNK.max(n))
                    .expect("descriptor buffer chunk");
                self.db.push(b);
            }
            self.db_used = 0;
        }
    }
    /// A stage set's place in the current chunk (after `db_reserve`): its offset.
    pub fn db_alloc(&mut self, size: u64) -> u64 {
        let off = self.db_used;
        self.db_used += size;
        // never past the chunk: the host copy would write outside the mapping and the GPU read outside the bound
        // descriptor buffer (callers reserve enough with `db_reserve`; this is the backstop)
        if self.db.get(self.db_cur).is_none_or(|c| self.db_used > c.size) {
            crate::log::red("descriptor buffer: a set would end past its chunk (reservation too small)");
            submission_safety::resource_safety_fatal();
        }
        off
    }
    /// A staging or scratch buffer of this submission's own.
    pub fn owns(&self, buf: vk::Buffer) -> bool {
        self.stage
            .iter()
            .chain(self.arena.iter())
            .any(|b| b.buf == buf)
    }
    /// The device address of a buffer of this submission's own (staging, scratch).
    pub fn addr_of(&self, buf: vk::Buffer) -> Option<u64> {
        self.stage
            .iter()
            .chain(self.arena.iter())
            .find(|b| b.buf == buf)
            .map(|b| b.addr)
            .filter(|&a| a != 0)
    }
    pub fn query(&mut self) -> u32 {
        if let Some(c) = &self.qshare {
            return c[0].fetch_add(1, Ordering::Relaxed);
        }
        let q = self.queries;
        self.queries += 1;
        q
    }
    /// A timestamp query of the pool's 256, if one is left.
    pub fn take_query(&mut self) -> Option<u32> {
        let q = if let Some(c) = &self.qshare {
            c[0].try_update(Ordering::Relaxed, Ordering::Relaxed, |n| (n < 256).then_some(n + 1))
                .ok()
        } else {
            (self.queries < 256).then(|| self.query())
        };
        if q.is_none() {
            queries_out(0);
        }
        q
    }
    /// An occlusion query of the pool's `occl_pool()`, if one is left.
    pub fn take_occl(&mut self) -> Option<u32> {
        let n_max = occl_pool();
        let q = if let Some(c) = &self.qshare {
            c[1].try_update(Ordering::Relaxed, Ordering::Relaxed, |n| (n < n_max).then_some(n + 1))
                .ok()
        } else {
            (self.occl_n < n_max).then(|| {
                self.occl_n += 1;
                self.occl_n - 1
            })
        };
        if q.is_none() {
            queries_out(1);
        }
        q
    }
}

/// `NXGFX_OCCL_POOL` (default 4096): occlusion queries a submission has. A query runs from a pass's (or a reset's)
/// first draw to the pass's end, its report or the next reset, so a game testing each object's box with its own reset
/// and report spends one a test: Ocarina of Time's Kokiri Forest and Link's house make ~560 tests a frame, and with 256
/// about 1,000 queries a frame were refused there, their draws missing from the counts (OOT-OCC). 256: as before.
pub fn occl_pool() -> u32 {
    static N: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("NXGFX_OCCL_POOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4096u32)
            .clamp(16, 65536)
    })
}

/// Queries refused because the submission's pool (256 timestamps, `occl_pool()` occlusion) was used up: timestamp
/// (counter reports), occlusion (audit L4: a parallel run's dropped children and in-pass cuts spend them faster).
/// Counted in the translate line.
pub static QUERIES_OUT: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

fn queries_out(k: usize) {
    QUERIES_OUT[k].fetch_add(1, Ordering::Relaxed);
    crate::log::red_once(if k == 0 {
        "a submission used all 256 timestamp queries: a counter report gets no GPU timestamp"
    } else {
        "a submission used all its occlusion queries (NXGFX_OCCL_POOL): a samples-passed count misses the draws after it"
    });
}

fn full_dependency() -> vk::MemoryBarrier2<'static> {
    vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
}

enum UploadEntry {
    Begin,
    Dependency(vk::MemoryBarrier2<'static>),
}

/// Production recording policy: no upload command may precede this dependency.
fn record_upload_entry(mut issue: impl FnMut(UploadEntry)) {
    issue(UploadEntry::Begin);
    issue(UploadEntry::Dependency(full_dependency()));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WritebackCopy { Result, Baseline }

/// Actual tail-copy recording plan. Baseline executes in this submission's `pre`,
/// result in its `main`; this does not recover a previous producer's baseline.
pub(crate) fn record_writeback_pair(has_baseline: bool, mut issue: impl FnMut(WritebackCopy)) {
    issue(WritebackCopy::Result);
    if has_baseline { issue(WritebackCopy::Baseline); }
}

fn barrier_all(g: &Gpu, cb: vk::CommandBuffer) {
    let mb = [full_dependency()];
    let di = vk::DependencyInfo::default().memory_barriers(&mb);
    unsafe { g.dev().cmd_pipeline_barrier2(cb, &di) };
}
pub fn full_barrier(g: &Gpu, cb: vk::CommandBuffer) {
    barrier_all(g, cb)
}

#[cfg(test)]
mod upload_entry_tests {
    use super::*;

    enum Command {
        Access(vk::PipelineStageFlags2, bool), // true = write
        Begin,
        Dependency(vk::MemoryBarrier2<'static>),
    }

    fn ordered(commands: &[Command]) -> bool {
        let Some((write, _)) = commands.iter().enumerate().rev()
            .find(|(_, c)| matches!(c, Command::Access(_, true))) else { return false; };
        let Some((prior, Command::Access(src, _))) = commands[..write].iter().enumerate().rev()
            .find(|(_, c)| matches!(c, Command::Access(_, _))) else { return true; };
        let Command::Access(dst, _) = commands[write] else { return false; };
        commands[prior + 1..write].iter().any(|c| match c {
            Command::Dependency(b) => {
                // WAR needs execution stage scopes; MEMORY_READ in src access is not required.
                (b.src_stage_mask.contains(vk::PipelineStageFlags2::ALL_COMMANDS) || b.src_stage_mask.contains(*src))
                    && (b.dst_stage_mask.contains(vk::PipelineStageFlags2::ALL_COMMANDS) || b.dst_stage_mask.contains(dst))
            }
            _ => false,
        })
    }

    #[test]
    fn actual_upload_entry_orders_terminal_readback_before_late_upload() {
        let transfer = vk::PipelineStageFlags2::TRANSFER;
        let mut commands = vec![
            Command::Access(vk::PipelineStageFlags2::COMPUTE_SHADER, true),
            Command::Dependency(full_dependency()), // accepted existing shader barrier
            Command::Access(transfer, false), // terminal A readback, AFTER that barrier
            Command::Dependency(vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)),
        ];
        let entry = commands.len();
        record_upload_entry(|event| commands.push(match event {
            UploadEntry::Begin => Command::Begin,
            UploadEntry::Dependency(b) => Command::Dependency(b),
        }));
        commands.push(Command::Access(transfer, true)); // B.pre late sync_range upload
        assert!(matches!(commands[entry], Command::Begin));
        assert!(ordered(&commands));
        let Command::Dependency(b) = &commands[entry + 1] else { panic!("entry dependency missing") };
        assert!(b.src_access_mask.contains(vk::AccessFlags2::MEMORY_WRITE));
        assert!(b.dst_access_mask.contains(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE));
        commands.remove(entry + 1); // old placement: only end-pre/main-entry, after first upload
        assert!(!ordered(&commands));
        commands.push(Command::Dependency(full_dependency()));
        assert!(!ordered(&commands), "a dependency after the upload is too late");

        let covered = [Command::Access(vk::PipelineStageFlags2::COMPUTE_SHADER, true),
            Command::Dependency(full_dependency()), Command::Access(transfer, true)];
        assert!(ordered(&covered), "ordinary post-shader coverage must not be called a new hazard");
    }
}

fn new_sub(g: &Gpu, st: &mut SubmitState) -> Sub {
    let reuse = st.free_subs.pop();
    make_sub(g, reuse, st.cmd_pool, st.main_pool, 0)
}

/// Translation lanes 1.. (`exec::run_segmented`): a command pool each (a pool's buffers may be recorded on one thread
/// at a time) and their free child submissions.
struct Lane {
    pool: vk::CommandPool,
    free: Mutex<Vec<Sub>>,
}
static LANES: Mutex<Vec<&'static Lane>> = Mutex::new(Vec::new());

fn lane(g: &Gpu, i: usize) -> &'static Lane {
    let mut l = LANES.lock().unwrap();
    while l.len() < i {
        let pci = vk::CommandPoolCreateInfo::default()
            .queue_family_index(g.d.qfam)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let pool = unsafe { g.dev().create_command_pool(&pci, None) }.expect("command pool");
        l.push(Box::leak(Box::new(Lane {
            pool,
            free: Mutex::new(Vec::new()),
        })));
    }
    l[i - 1]
}

/// A child submission recorded on translation lane `i` (1..), from that lane's command pool.
pub fn lane_sub(g: &Gpu, i: usize) -> Sub {
    let l = lane(g, i);
    let reuse = {
        let mut f = l.free.lock().unwrap();
        trim_free(g, &mut f, (sub_keep() / 4).max(1));
        f.pop()
    };
    make_sub(g, reuse, l.pool, l.pool, i)
}

fn make_sub(
    g: &Gpu,
    reuse: Option<Sub>,
    pre_pool: vk::CommandPool,
    main_pool: vk::CommandPool,
    lane: usize,
) -> Sub {
    let mut s = if let Some(s) = reuse {
        s
    } else {
        let ai = vk::CommandBufferAllocateInfo::default()
            .command_pool(pre_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let pre = unsafe { g.dev().allocate_command_buffers(&ai) }.expect("command buffers")[0];
        let ai = vk::CommandBufferAllocateInfo::default()
            .command_pool(main_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let main = unsafe { g.dev().allocate_command_buffers(&ai) }.expect("command buffers")[0];
        let cbs = [pre, main];
        let qi = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(256);
        let query = unsafe { g.dev().create_query_pool(&qi, None) }.expect("query pool");
        let oi = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::OCCLUSION)
            .query_count(occl_pool());
        let occl = unsafe { g.dev().create_query_pool(&oi, None) }.expect("occlusion query pool");
        Sub {
            pre: cbs[0],
            main: cbs[1],
            stage: Vec::new(),
            stage_used: 0,
            dpools: Default::default(),
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
            pool_snapshot: None,
            serial: 0,
            pre_serial: 0,
            frame: 0,
            gmem: None,
            db: Vec::new(),
            db_cur: 0,
            db_used: 0,
            db_bound: None,
            db_log: Vec::new(),
            children: Vec::new(),
            own_queries: None,
            lane,
            qshare: None,
            dropped: Vec::new(),
        }
    };
    static SUB_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    s.serial = SUB_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    s.pre_serial = s.serial;
    s.frame = super::present::frames();
    g.q.recording.store(true, Ordering::Release);
    let bi =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe {
        g.dev()
            .reset_command_buffer(s.pre, vk::CommandBufferResetFlags::empty())
            .unwrap();
        g.dev()
            .reset_command_buffer(s.main, vk::CommandBufferResetFlags::empty())
            .unwrap();
        // A predecessor's terminal writeback copy reads a mirrored pool after
        // its last shader/pass barrier. Late cold-pool sync_range uploads bypass
        // ordinary begin_submit's in-flight deferral and are recorded in `pre`.
        // Order those writes after the predecessor tail, before the first upload.
        record_upload_entry(|entry| match entry {
            UploadEntry::Begin => { g.dev().begin_command_buffer(s.pre, &bi).unwrap(); }
            UploadEntry::Dependency(barrier) => {
                g.dev().cmd_pipeline_barrier2(s.pre,
                    &vk::DependencyInfo::default().memory_barriers(&[barrier]));
            }
        });
        g.dev().begin_command_buffer(s.main, &bi).unwrap();
        super::defer::begin(s.main);
        g.dev().cmd_reset_query_pool(s.pre, s.query, 0, 256);
        g.dev().cmd_reset_query_pool(s.pre, s.occl, 0, occl_pool());
        g.dev()
            .cmd_write_timestamp2(s.main, vk::PipelineStageFlags2::TOP_OF_PIPE, s.query, 0);
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
    let hold = *HOLD.get_or_init(|| {
        std::env::var("NXGFX_STAGE_HOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    });
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

/// A child submission (`new_sub`) starts a segment of `parent`'s stream: its timestamp and occlusion queries go into
/// the parent's pools after the parent's (completion reads them there, with the posts that name them), and it binds
/// the parent's texture pool snapshot.
pub fn child_begin(parent: &Sub, c: &mut Sub) {
    child_begin_with(&child_init(parent), c);
}

/// What children translated at the same time take from their parent (`child_begin_with`); the parent's query counts
/// come back with `child_settle` once they are all translated.
#[derive(Clone)]
pub struct ChildInit {
    query: vk::QueryPool,
    occl: vk::QueryPool,
    pool_snapshot: Option<(u64, vk::DescriptorSet, u64)>,
    share: std::sync::Arc<[std::sync::atomic::AtomicU32; 2]>,
    /// the parent's upload buffer and serial (`share_pre`)
    pre: vk::CommandBuffer,
    pre_serial: u64,
}
unsafe impl Send for ChildInit {}
unsafe impl Sync for ChildInit {}

pub fn child_init(parent: &Sub) -> ChildInit {
    use std::sync::atomic::AtomicU32;
    ChildInit {
        query: parent.query,
        occl: parent.occl,
        pool_snapshot: parent.pool_snapshot,
        share: std::sync::Arc::new([AtomicU32::new(parent.queries), AtomicU32::new(parent.occl_n)]),
        pre: parent.pre,
        pre_serial: parent.pre_serial,
    }
}

/// A parallel run's segment records its first-use initializations into its parent's upload buffer, shared by every
/// segment of the run and recorded under `gate::pre_lock`, in the order they are made (audit H1: in each child's own
/// upload buffer they ran in segment order, so a page uploaded by an older segment could be overwritten by a younger
/// segment's earlier upload of the whole pool, and an image's twin copied before the image's first upload). Returns
/// the child's own upload buffer, for `unshare_pre` once the segment is translated.
pub fn share_pre(ci: &ChildInit, c: &mut Sub) -> vk::CommandBuffer {
    c.pre_serial = ci.pre_serial;
    std::mem::replace(&mut c.pre, ci.pre)
}

/// The child's own upload buffer back (`share_pre`); it holds no initialization, only its start.
pub fn unshare_pre(c: &mut Sub, own: vk::CommandBuffer) {
    c.pre = own;
    c.pre_serial = c.serial;
}

pub fn child_begin_with(ci: &ChildInit, c: &mut Sub) {
    c.own_queries = Some((c.query, c.occl));
    c.query = ci.query;
    c.occl = ci.occl;
    c.qshare = Some(ci.share.clone());
    c.pool_snapshot = ci.pool_snapshot;
    c.db_bound = None;
}

/// The parent's query counts after its children's (`child_begin_with`).
pub fn child_settle(parent: &mut Sub, ci: &ChildInit) {
    parent.queries = ci.share[0].load(Ordering::Relaxed).min(256);
    parent.occl_n = ci.share[1].load(Ordering::Relaxed).min(occl_pool());
}

/// A child's segment is translated: its CPU-side effects join the parent's, in stream order, and it is kept with it.
pub fn child_join(parent: &mut Sub, mut c: Sub) {
    if let Some(s) = &c.qshare {
        parent.queries = s[0].load(Ordering::Relaxed).min(256);
        parent.occl_n = s[1].load(Ordering::Relaxed).min(occl_pool());
    }
    parent.post.append(&mut c.post);
    super::mem::join_writebacks(parent, std::mem::take(&mut c.writebacks));
    parent.tex_wb.append(&mut c.tex_wb);
    if c.pool_snapshot.is_some() {
        parent.pool_snapshot = c.pool_snapshot;
    }
    parent.children.push(c);
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
    wait_diag::phase("submit: writebacks");
    let q = &g.q;
    // segments in child submissions (`exec::run_segmented`): the last child's buffers are the submission's last, so
    // what is recorded below at the end of `pre` (sparse fills, write-back baselines) and of `main` (write-back copies,
    // the end timestamp) goes there, after every child's work; swapped back once submitted
    let nchild = s.children.len();
    if nchild > 0 {
        let l = &mut s.children[nchild - 1];
        std::mem::swap(&mut s.pre, &mut l.pre);
        std::mem::swap(&mut s.main, &mut l.main);
    }
    super::gmem::end(g, &mut s);
    // write-backs: copy the GPU-written ranges into one readback buffer at the end of the replay
    exec::seg_stats::WB_RANGES.fetch_add(s.writebacks.len() as u64, Ordering::Relaxed);
    let total: u64 = s.writebacks.iter().map(|w| (w.2 + 15) & !15).sum();
    let mut readback = None;
    let mut before: Option<Buf> = None;
    let mut wb = Vec::new();
    if total > 0 {
        barrier_all(g, s.main);
        let rb = wb_bufs::take(g, total, "write-back").expect("readback");
        // the same ranges as the device had them before this submission's work (after its uploads, in `pre`): only
        // the words the GPU changed land in guest memory, so what the CPU wrote beside them meanwhile stays.
        // Landing the whole range put back stale bytes the CPU had changed since the upload: a
        // storage buffer bound read-write over a pool the CPU also writes (a size-0 one runs to the end of its pool)
        let bf = wb_bufs::take(g, total, "write-back base").ok();
        if bf.is_some() {
            barrier_all(g, s.pre);
        }
        let mut off = 0;
        super::lost::checkpoint(g, s.main, false, || {
            format!("the write-back copies of {} range(s)", s.writebacks.len())
        });
        for &(pool, a, n, buf, boff, identity) in &s.writebacks {
            let r = [vk::BufferCopy {
                src_offset: boff,
                dst_offset: off,
                size: n,
            }];
            record_writeback_pair(bf.is_some(), |copy| match copy {
                WritebackCopy::Result => unsafe { g.dev().cmd_copy_buffer(s.main, buf, rb.buf, &r) },
                WritebackCopy::Baseline => {
                    if let Some(b) = &bf {
                        unsafe { g.dev().cmd_copy_buffer(s.pre, buf, b.buf, &r) };
                    }
                }
            });
            let base = bf.as_ref().map_or(core::ptr::null(), |b| unsafe {
                b.ptr.add(off as usize) as *const u8
            });
            wb.push((
                pool,
                a,
                n,
                unsafe { rb.ptr.add(off as usize) } as *const u8,
                base,
                identity,
            ));
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
    unsafe {
        g.dev()
            .cmd_pipeline_barrier2(s.main, &vk::DependencyInfo::default().memory_barriers(&mb))
    };
    wait_diag::phase("submit: sparse fills/end command buffers");
    super::sparse::fills(g, s.pre);
    barrier_all(g, s.pre);
    unsafe {
        g.dev()
            .cmd_write_timestamp2(s.main, vk::PipelineStageFlags2::ALL_COMMANDS, s.query, 1);
        g.dev().end_command_buffer(s.pre).unwrap();
        super::defer::end(s.main);
        g.dev().end_command_buffer(s.main).unwrap();
        for c in &s.children {
            g.dev().end_command_buffer(c.pre).unwrap();
            super::defer::end(c.main);
            g.dev().end_command_buffer(c.main).unwrap();
        }
        // a dropped child's upload buffer: begun, so ended and submitted with the others (a parallel segment's
        // initializations are in the shared one, `share_pre`); its main buffer is not
        for c in &s.dropped {
            g.dev().end_command_buffer(c.pre).unwrap();
        }
    }
    wait_diag::phase("submit: driver/queue lock");
    let _x = super::pipe::driver_exclusive(g).then(|| super::pipe::EXCLUSIVE.lock().unwrap());
    let _vq = q.vkq.lock().unwrap();
    let value = q.last.load(Ordering::Acquire) + 1;
    // in stream order: the parent's buffers (now in the last child), the children's, the last child's (now in `s`);
    // every upload buffer before every main buffer, as one submission's single `pre` and `main`
    let order = |pick: fn(&Sub) -> vk::CommandBuffer| -> Vec<vk::CommandBufferSubmitInfo> {
        let mut v: Vec<vk::CommandBuffer> = Vec::with_capacity(nchild + 1);
        if nchild > 0 {
            v.push(pick(&s.children[nchild - 1]));
            v.extend(s.children[..nchild - 1].iter().map(pick));
        }
        v.push(pick(&s));
        v.into_iter()
            .map(|cb| vk::CommandBufferSubmitInfo::default().command_buffer(cb))
            .collect()
    };
    let mut cbs = order(|x| x.pre);
    cbs.splice(
        1..1,
        s.dropped
            .iter()
            .map(|c| vk::CommandBufferSubmitInfo::default().command_buffer(c.pre)),
    );
    cbs.extend(order(|x| x.main));
    let mut sig = vec![vk::SemaphoreSubmitInfo::default()
        .semaphore(q.timeline)
        .value(value)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    let mut wait = vec![];
    // sparse binds changed since the last submission go first (after every earlier submission); this one waits for them
    wait_diag::phase("submit: sparse flush");
    if let Some((sem, v)) = super::sparse::flush(g, value - 1, q.timeline, value) {
        wait.push(
            vk::SemaphoreSubmitInfo::default()
                .semaphore(sem)
                .value(v)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        );
    }
    for p in &s.present {
        wait.push(
            vk::SemaphoreSubmitInfo::default()
                .semaphore(p.acquired)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        );
        sig.push(
            vk::SemaphoreSubmitInfo::default()
                .semaphore(p.rendered)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        );
    }
    let si = [vk::SubmitInfo2::default()
        .command_buffer_infos(&cbs)
        .signal_semaphore_infos(&sig)
        .wait_semaphore_infos(&wait)];
    wait_diag::phase("submit: vkQueueSubmit2");
    super::newndve::trace::link(s.serial, value, false);
    // Optional bounded pre-fault export through the existing logger. Never entered
    // from fatal72; snapshot releases its try-lock before the sink is called.
    if std::env::var("NXGFX_NATIVE_TRACE").as_deref() == Ok("1") {
        if let Some((records, overwritten, omitted)) = super::newndve::trace::snapshot(s.serial) {
            if !records.is_empty() {
                crate::vklog!("native recording {} -> timeline {value} attempted: overwritten={overwritten} omitted={omitted} {records:?}", s.serial);
            }
        }
    }
    unsafe { g.dev().queue_submit2(g.d.queue, &si, vk::Fence::null()) }.unwrap_or_else(|e| {
        super::lost::check(g, e, "vkQueueSubmit2");
        panic!("vkQueueSubmit2: {e}")
    });
    q.last.store(value, Ordering::Release);
    chain_trace::note("vk_submit", value, s.present.len() as u64);
    super::lost::ring::note_submit(s.serial, value);
    for c in &s.children {
        super::lost::ring::note_submit(c.serial, value);
    }
    if nchild > 0 {
        let l = &mut s.children[nchild - 1];
        std::mem::swap(&mut s.pre, &mut l.pre);
        std::mem::swap(&mut s.main, &mut l.main);
    }
    super::newndve::trace::link(s.serial, value, true);
    {
        let mut l = q.after_current.lock().unwrap();
        let mut d = q.deferred.lock().unwrap();
        for f in l.drain(..) {
            d.push((value, f));
        }
        q.recording.store(false, Ordering::Release);
    }
    wait_diag::phase("submit: present/enqueue completion");
    // every GPU-write note of this recording is made: its landing re-arms the pages stamped with this epoch
    let epoch = super::upskip::next_epoch();
    let presents = std::mem::take(&mut s.present);
    if presents.len() == 1 {
        super::present::queue_present(g, presents.into_iter().next().unwrap());
    } else if !presents.is_empty() {
        // generated frames: spaced over the game's frame interval by frame generation's presenting thread
        drop(_vq);
        super::framegen::schedule(g, presents);
        start_completion(g);
        q.inflight.lock().unwrap().push_back(Inflight {
            value,
            sub: s,
            wb,
            readback,
            before,
            epoch,
        });
        q.inflight_cv.notify_one();
        return value;
    }
    drop(_vq);
    start_completion(g);
    q.inflight.lock().unwrap().push_back(Inflight {
        value,
        sub: s,
        wb,
        readback,
        before,
        epoch,
    });
    q.inflight_cv.notify_one();
    value
}

/// The host buffers a submission's write-backs are copied into (the GPU-written ranges and their baseline), kept for
/// the next submissions instead of freed at completion (OOT-DESC). Made and freed per submission, each one was a fresh
/// block of host memory from the driver (`vkAllocateMemory`, ~15 % of Ocarina of Time's replay thread, and its
/// `vkFreeMemory` ~6 % of the completion thread's). A kept buffer is handed out again only after the completion of the
/// submission that used it has landed its write-backs (`give` runs after `apply_writebacks`), and a submission writes
/// every byte of the ranges it reads back (`submit_sub`). `NXGFX_WB_RECYCLE=0`: made and freed each time, as before.
pub(crate) mod wb_bufs {
    use super::{Buf, Gpu};
    use ash::vk;
    use gpu_allocator::MemoryLocation;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    /// Bytes kept at most, and buffers of one size at most; a buffer that would pass either is freed. (Ocarina of Time
    /// writes back ranges of 32-64 MB a few times a frame, readback and baseline each: in the house 4-6 buffers of 64 MB
    /// are in flight at once. A 64 MB cap left every small buffer to be made again (od-wb1-oot), a 256 MB one still made
    /// ~one 64 MB buffer a frame at the peaks (od-v4-oot).)
    const KEEP: u64 = 1 << 30;
    const PER_SIZE: usize = 64;
    /// The smallest buffer made.
    const MIN: u64 = 64 << 10;

    pub(crate) fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("NXGFX_WB_RECYCLE").as_deref() != Ok("0"))
    }

    /// The size a buffer for `n` bytes is made at: a power of two from 64 KiB, so nearby sizes serve each other.
    pub(crate) fn class(n: u64) -> u64 {
        n.max(MIN).next_power_of_two()
    }

    /// Kept buffers with their sizes; the smallest one holding the bytes asked for is taken.
    pub(crate) struct Shelf<T> {
        items: Vec<(u64, T)>,
        bytes: u64,
    }

    impl<T> Shelf<T> {
        pub(crate) const fn new() -> Self {
            Shelf {
                items: Vec::new(),
                bytes: 0,
            }
        }
        /// The smallest kept item of at least `n` bytes.
        pub(crate) fn take(&mut self, n: u64) -> Option<T> {
            let i = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, (s, _))| *s >= n)
                .min_by_key(|(_, (s, _))| *s)
                .map(|(i, _)| i)?;
            let (s, t) = self.items.swap_remove(i);
            self.bytes -= s;
            Some(t)
        }
        /// Keeps `t` of `size` bytes, or hands it back when the shelf would hold more than `keep` bytes or more than
        /// `per_size` items of that size.
        pub(crate) fn put(&mut self, size: u64, t: T, keep: u64, per_size: usize) -> Option<T> {
            if self.bytes + size > keep
                || self.items.iter().filter(|(s, _)| *s == size).count() >= per_size
            {
                return Some(t);
            }
            self.bytes += size;
            self.items.push((size, t));
            None
        }
        pub(crate) fn bytes(&self) -> u64 {
            self.bytes
        }
    }

    static SHELF: Mutex<Shelf<Buf>> = Mutex::new(Shelf::new());
    /// Buffers made, and buffers handed out again (the memory line).
    pub(crate) static MADE: AtomicU64 = AtomicU64::new(0);
    pub(crate) static REUSED: AtomicU64 = AtomicU64::new(0);

    /// A mapped host buffer of at least `n` bytes for a submission's write-back copies.
    pub(crate) fn take(g: &Gpu, n: u64, name: &str) -> Result<Buf, String> {
        if !on() {
            return g.buffer(n, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, name);
        }
        if let Some(b) = SHELF.lock().unwrap().take(n) {
            REUSED.fetch_add(1, Ordering::Relaxed);
            return Ok(b);
        }
        MADE.fetch_add(1, Ordering::Relaxed);
        g.buffer(class(n), vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, name)
    }

    /// A completed submission's write-back buffer: kept for the next ones (or freed past the cap, or with the switch off).
    pub(crate) fn give(g: &Gpu, b: Buf) {
        if !on() {
            g.free_buffer(b);
            return;
        }
        let size = b.size;
        let back = SHELF.lock().unwrap().put(size, b, KEEP, PER_SIZE);
        if let Some(b) = back {
            g.free_buffer(b);
        }
    }

    /// `write-back buffers N made, M reused, K MB kept`.
    pub(crate) fn line() -> String {
        format!(
            "write-back buffers {} made, {} reused, {} MB kept",
            MADE.load(Ordering::Relaxed),
            REUSED.load(Ordering::Relaxed),
            SHELF.lock().unwrap().bytes() >> 20
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The smallest kept buffer that holds the bytes is handed out; nothing past the cap is kept.
        #[test]
        fn the_smallest_fitting_buffer_is_reused_and_the_cap_holds() {
            let mut s: Shelf<&str> = Shelf::new();
            assert_eq!(s.put(class(100_000), "128k", 1 << 20, 4), None);
            assert_eq!(s.put(class(10), "64k", 1 << 20, 1), None);
            assert_eq!(s.put(class(600_000), "1m", 1 << 20, 4), Some("1m"), "past the cap: handed back");
            assert_eq!(s.put(class(20), "64k again", 1 << 20, 1), Some("64k again"), "one of a size at most");
            assert_eq!(s.bytes(), (128 << 10) + (64 << 10));
            assert_eq!(s.take(70_000), Some("128k"), "64 KiB is too small");
            assert_eq!(s.take(200_000), None, "nothing large enough");
            assert_eq!(s.take(1), Some("64k"));
            assert_eq!(s.bytes(), 0);
            assert_eq!(class(0), 64 << 10);
            assert_eq!(class((64 << 10) + 1), 128 << 10);
        }
    }
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
        wait_diag::mark(2, "idle", [0, 0]);
        let mut item = {
            let mut l = q.inflight.lock().unwrap();
            while l.is_empty() {
                l = q.inflight_cv.wait(l).unwrap();
            }
            l.pop_front().unwrap()
        };
        // the web device's worker does on its own thread the CPU work a native driver does inside vkQueueSubmit
        // (command encoding, WebGPU pipelines created at their first draw): the GPU-time budget below starts once it
        // has handed this submission to the GPU (6.8 s for a cold-cache submission ran out the budget, and the fatal
        // ended only this thread: the game waited for ever, WEB-W4c)
        #[cfg(target_os = "emscripten")]
        {
            wait_diag::mark(2, "web worker execute wait", [item.value, 0]);
            let t = crate::web::icd::wait_queued(q.timeline, item.value);
            if t.as_secs() >= 5 {
                crate::vklog!("web: the GPU worker took {:.1} s to execute submission {} before the GPU could start it (pipeline creation)", t.as_secs_f64(), item.value);
            }
        }
        wait_diag::mark(2, "GPU timeline wait", [item.value, 0]);
        let sems = [q.timeline];
        let vals = [item.value];
        let wi = vk::SemaphoreWaitInfo::default()
            .semaphores(&sems)
            .values(&vals);
        let started = std::time::Instant::now();
        let completion = submission_safety::wait_for_completion(
            item.value,
            || started.elapsed(),
            |slice| match unsafe { g.dev().wait_semaphores(&wi, slice.as_nanos() as u64) } {
                Ok(()) => Ok(true),
                Err(vk::Result::TIMEOUT) => Ok(false),
                Err(e) => Err(e),
            },
        );
        // the web build: the fatal below is an abort with no reason (native's exit status 72 reaches no one there), so
        // the page said only "The game stopped" (WEB-W8c, run after3): name the submission and why first
        #[cfg(target_os = "emscripten")]
        if let Err(e) = &completion {
            let why = match e {
                submission_safety::CompletionError::Deadline => format!(
                    "not complete {:.1} s after the GPU worker handed it to the GPU",
                    started.elapsed().as_secs_f64()
                ),
                submission_safety::CompletionError::Driver(r) => format!("the wait failed: {r:?}"),
            };
            crate::log::red(&format!("web: submission {} {why}; the game stops (GPU completion budget)", item.value));
        }
        let completion = submission_safety::resource_safety_result(completion);
        super::lost::ring::COMPLETED.store(item.value, Ordering::Relaxed);
        chain_trace::note("gpu_done", item.value, item.wb.len() as u64);
        // Failure above terminates with status 72, without entering any logger/cleanup.
        // Userspace still cannot preempt a driver call that never returns or prevent TDR.
        let mut signalled = false;
        // effects that must precede what the game can see of this submission (raw global memory's write log)
        for p in item.sub.post.iter_mut() {
            if matches!(p, Post::Early(_)) {
                if let Post::Early(f) = std::mem::replace(p, Post::Callback(Box::new(|_: &Gpu| {}))) {
                    f(g, item.value);
                }
            }
        }
        // CPU-visible effects, in order: write-backs first (an event may tell the game to read them)
        wait_diag::mark(2, "CPU writebacks", [item.value, 0]);
        super::mem::apply_writebacks(g, &item.wb, item.epoch, item.value);
        wait_diag::mark(2, "queries/post effects", [item.value, 0]);
        let mut sub = item.sub;
        if super::exec::db_poison_on() {
            super::exec::db_canary_scan(g, &sub, item.value);
        }
        let ts: Vec<u64> = {
            let mut v = vec![0u64; sub.queries.max(2) as usize];
            unsafe {
                g.dev()
                    .get_query_pool_results(sub.query, 0, &mut v, vk::QueryResultFlags::TYPE_64)
            }
            .ok();
            let period = g.timestamp_period() as f64;
            v.iter().map(|&t| (t as f64 * period) as u64).collect()
        };
        let (t0, t1) = (ts[0], ts[1]);
        if t1 >= t0 && t1 > 0 {
            // the submission's own GPU time (chain trace: gpu_done less vk_submit = queueing + this + the wake)
            chain_trace::note("gpu_ts", item.value, t1 - t0);
            super::counters::part(super::counters::part_key(sub.frame, sub.serial), t0, t1);
            super::perf::gpu_time(t1 - t0);
            super::scale::note_gpu(t1 - t0);
        }
        let occl: Vec<u64> = if sub.occl_n > 0 {
            let mut v = vec![0u64; sub.occl_n as usize];
            unsafe {
                g.dev()
                    .get_query_pool_results(sub.occl, 0, &mut v, vk::QueryResultFlags::TYPE_64)
            }
            .ok();
            v
        } else {
            vec![]
        };
        for p in sub.post.drain(..) {
            match p {
                Post::Event { addr, mode, value } => {
                    chain_trace::note("gpu_sig", addr, (mode as u64) << 32 | value as u64);
                    crate::nvn::sync::event_publish(addr as *mut u32, mode, value, false);
                    signalled = true;
                }
                Post::Counter { addr, kind, query } => {
                    let stamp = if query != u32::MAX {
                        ts.get(query as usize).copied()
                    } else {
                        None
                    };
                    let t = super::counters::report_time(stamp, t0, t1);
                    // a timestamp's counter field is 0 (agl's "already cleared" records compare it), other kinds 0 too
                    let ts = kind == crate::nvn::types::counter::TIMESTAMP
                        || kind == crate::nvn::types::counter::TIMESTAMP_TOP;
                    super::counters::write_record(addr, 0, t);
                    if ts {
                        super::counters::rc_landed(addr);
                    }
                }
                Post::Occl { q } => {
                    super::counters::vis_add(occl.get(q as usize).copied().unwrap_or(0))
                }
                Post::OcclReset => super::counters::vis_reset(),
                Post::OcclReport { addr } => {
                    let t = super::counters::report_time(None, t0, t1);
                    super::occl::land(addr, super::counters::vis(), t);
                }
                Post::Readback { dst, src, n } => unsafe {
                    core::ptr::copy_nonoverlapping(src, dst, n)
                },
                Post::Callback(f) => f(g),
                Post::Early(f) => f(g, item.value),
            }
        }
        chain_trace::note("cpl_land", item.value, 0);
        wait_diag::mark(2, "recycle/deferred callbacks", [item.value, 0]);
        // the write-backs have landed (`apply_writebacks` above): nothing reads these buffers any more
        if let Some(rb) = item.readback {
            wb_bufs::give(g, rb);
        }
        if let Some(b) = item.before {
            wb_bufs::give(g, b);
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
        sub.dpools
            .recycle(
                item.value,
                &completion,
                |p| {
                    unsafe {
                        g.dev()
                            .reset_descriptor_pool(*p, vk::DescriptorPoolResetFlags::empty())
                    }
                    .ok();
                },
                |p| unsafe { g.dev().destroy_descriptor_pool(p, None) },
            )
            .unwrap_or_else(|_| submission_safety::resource_safety_fatal());
        super::newndve::trace::retire(sub.serial, item.value);
        sub.dpool_i = 0;
        // segments' child submissions (and those translated again, never submitted): their resources as the parent's,
        // then back to their lane's free list
        let dropped = std::mem::take(&mut sub.dropped);
        for mut c in std::mem::take(&mut sub.children).into_iter().chain(dropped) {
            for b in c.transient.drain(..) {
                g.free_buffer(b);
            }
            c.arena_used = 0;
            while c.arena.len() > 1 || c.arena.last().is_some_and(|b| b.size > ARENA_CHUNK) {
                let b = c.arena.pop().unwrap();
                g.free_buffer(b);
            }
            for v in c.transient_views.drain(..) {
                unsafe { g.dev().destroy_image_view(v, None) };
            }
            for v in c.buffer_views.drain(..) {
                unsafe { g.dev().destroy_buffer_view(v, None) };
            }
            c.dpools
                .recycle(
                    item.value,
                    &completion,
                    |p| {
                        unsafe {
                            g.dev()
                                .reset_descriptor_pool(*p, vk::DescriptorPoolResetFlags::empty())
                        }
                        .ok();
                    },
                    |p| unsafe { g.dev().destroy_descriptor_pool(p, None) },
                )
                .unwrap_or_else(|_| submission_safety::resource_safety_fatal());
            super::newndve::trace::retire(c.serial, item.value);
            c.dpool_i = 0;
            if let Some((qp, op)) = c.own_queries.take() {
                c.query = qp;
                c.occl = op;
            }
            c.db_cur = 0;
            c.db_used = 0;
            c.db_bound = None;
            c.memo_bytes.clear();
            c.memo_sets.clear();
            c.memo_views.clear();
            c.pool_snapshot = None;
            c.writebacks.clear();
            c.tex_wb.clear();
            c.post.clear();
            c.queries = 2;
            c.occl_n = 0;
            c.qshare = None;
            if c.lane > 0 {
                lane(g, c.lane).free.lock().unwrap().push(c);
            } else {
                q.recycled.lock().unwrap().push(c);
            }
        }
        // NXGFX_RECYCLE_DELAY=<n> (diagnosis): a completed submission's `Sub` waits until n later ones completed before
        // it is poisoned (NXGFX_DB_POISON) and reused
        static DELAY: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        static HELD: Mutex<VecDeque<Sub>> = Mutex::new(VecDeque::new());
        let delay = *DELAY.get_or_init(|| {
            std::env::var("NXGFX_RECYCLE_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
        });
        let mut sub = if delay > 0 {
            let mut h = HELD.lock().unwrap();
            h.push_back(sub);
            if h.len() <= delay {
                None
            } else {
                h.pop_front()
            }
        } else {
            Some(sub)
        };
        if let Some(sub) = sub.as_mut() {
            if super::exec::db_poison_on() {
                super::exec::db_poison(g, sub);
            }
        }
        if let Some(mut sub) = sub {
        sub.db_cur = 0;
        sub.db_used = 0;
        sub.db_bound = None;
        sub.memo_bytes.clear();
        sub.memo_sets.clear();
        sub.memo_views.clear();
        sub.pool_snapshot = None;
        sub.writebacks.clear();
        sub.tex_wb.clear();
        sub.queries = 2;
        sub.occl_n = 0;
        q.recycled.lock().unwrap().push(sub);
        }
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
        wait_diag::mark(2, "publish done/wake", [item.value, 0]);
        q.pending_signals
            .lock()
            .unwrap()
            .retain(|_, v| v.2 > item.value);
        *q.done.lock().unwrap() = item.value;
        chain_trace::note("cpl_done", item.value, signalled as u64);
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
                        let (ff, to) =
                            g.q.retry_cv
                                .wait_timeout(f, std::time::Duration::from_secs(1))
                                .unwrap();
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
    drop(r);
    trim_free(g, &mut st.free_subs, sub_keep());
}

/// Free submissions that keep their staging and scratch chunks: `NXGFX_SUB_KEEP` (default 64; 0 = all, as before).
/// OOT-COMBO: a submission keeps one 8 MB staging chunk and one 64 MB scratch chunk for its next use, and pooled
/// submissions are never freed, so Ocarina of Time's forest-load burst (~415 submissions) left ~3.5 GB of staging and
/// ~2.6 GB of scratch held for the rest of the run.
fn sub_keep() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("NXGFX_SUB_KEEP").ok().and_then(|v| v.parse().ok()).unwrap_or(64)
    })
}

fn sub_idle() -> u64 {
    static N: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("NXGFX_SUB_IDLE").ok().and_then(|v| v.parse().ok()).unwrap_or(300)
    })
}

/// The free submissions past the newest `keep` (the front of the list: `pop` takes from the back) give back their
/// staging and scratch chunks; a later use allocates them again. They are free, so the GPU is done with them. Each
/// translation lane's own list keeps a quarter of `sub_keep()` (six lists of 64 still held ~4.9 GB of staging in the
/// house: OoT's process then reached its 32 GB cap). Bytes given back.
/// (OOT-MEM) Kept ones not used for `NXGFX_SUB_IDLE` presents (default 300; 0 = kept however long) give theirs back
/// too: `pop` takes from the back, so most of the 64 (and each lane's 16) sat idle with a 64 MB scratch chunk each
/// (OoT's house: 55 chunks, 3.5 GB, of which a frame uses a few).
fn trim_free(g: &Gpu, free: &mut [Sub], keep: usize) -> u64 {
    if sub_keep() == 0 {
        return 0;
    }
    let n = free.len().saturating_sub(keep);
    let (now, idle) = (super::present::frames(), sub_idle());
    let mut bytes = 0;
    for (i, s) in free.iter_mut().enumerate() {
        if i >= n && (idle == 0 || now.saturating_sub(s.frame) <= idle) {
            continue;
        }
        for b in s.stage.drain(..) {
            bytes += b.size;
            g.free_buffer(b);
        }
        for b in s.arena.drain(..) {
            bytes += b.size;
            g.free_buffer(b);
        }
        s.stage_used = 0;
        s.arena_used = 0;
    }
    if bytes >= 256 << 20 {
        crate::vklog!(
            "pooled submissions: {} MB of staging and scratch given back ({} free, {} keep theirs; NXGFX_SUB_KEEP)",
            bytes >> 20,
            free.len(),
            keep
        );
    }
    bytes
}

// ---------- asynchronous submission ----------
//
// The game's thread only copies a submission's records (or notes a present) and goes on; the replay thread records
// and submits them in order (the translation thread). Fences the game takes meanwhile name the op (ASYNC | its
// sequence number) and resolve to its timeline value once it ran. `NXGFX_ASYNC=0`: replay on the game's thread.

const ASYNC: u64 = 1 << 62;

enum Op {
    Submit {
        q: u64,
        ll: bool,
        stream: Vec<u8>,
    },
    Present {
        q: u64,
        ll: bool,
        t: crate::nvn::obj::tex_t,
        crop: Option<[i32; 4]>,
    },
    Call(Box<dyn FnOnce(&'static Gpu) + Send>),
    Retry,
    WaitReceipt {
        q: u64,
        r: u64,
    },
}

/// Queue `q` waits for receipt `r` (a sync of another queue's parked work) before anything else it was given.
fn park_on_receipt(st: &mut SubmitState, q: u64, r: u64) {
    match st.parked.get_mut(&q) {
        Some(p) if p.wait_receipt.is_none() && p.stream.is_empty() && p.later.is_empty() => {
            p.wait_receipt = Some(r)
        }
        // already parked with work: the wait sits at the end of that work (the work queued after it runs once it passed)
        Some(p) => p.later.push((p.stream.len(), r)),
        None => {
            st.parked.insert(
                q,
                Parked {
                    stream: Vec::new(),
                    receipts: Vec::new(),
                    wait_receipt: Some(r),
                    presents: Vec::new(),
                    later: Vec::new(),
                },
            );
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
        // on by default: Pokemon Legends Z-A in Lumiose 49.5-52.4 FPS against 42.5-48 replayed on the
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
        std::thread::Builder::new()
            .name("vk replay".into())
            .spawn(move || aq_main(g, a))
            .expect("replay thread");
    }
    a
}

/// Ops queued and not replayed at most (AQ_N), presents among them at most (AQ_PRESENTS: frames in flight).
const AQ_MAX: u64 = 64;
/// `NXGFX_ASYNC_PRESENTS` (default 2): presents queued at most.
fn aq_max_presents() -> u64 {
    static N: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("NXGFX_ASYNC_PRESENTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2u64)
            .max(1)
    })
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
    // FPS, and did not help Mario Kart 8 Deluxe). NXGFX_OCCL_LAG (occl.rs, default 1): the replay may stay that many
    // presents behind instead (0: the wait above), so the game builds its next frame while the replay translates this
    // one and reads the most recent results completed (Ocarina of Time's render thread waited here 83 % of its time)
    let occl = present && occl_sync();
    let lag = super::occl::lag();
    let full = || {
        let behind = a.tail.load(Ordering::Acquire) - a.done.load(Ordering::Acquire);
        let queued = AQ_PRESENTS.load(Ordering::Acquire);
        behind >= AQ_MAX - 1
            || (present && queued >= aq_max_presents())
            || (occl && super::occl::present_waits(lag, queued, behind))
    };
    if !full() {
        return;
    }
    let t0 = std::time::Instant::now();
    let by_presents = present
        && a.tail.load(Ordering::Acquire) - a.done.load(Ordering::Acquire) < AQ_MAX - 1;
    let mut l = a.idle_mu.lock().unwrap();
    while full() {
        l = a
            .idle_cv
            .wait_timeout(l, std::time::Duration::from_millis(20))
            .unwrap()
            .0;
    }
    drop(l);
    super::perf::add(
        if by_presents {
            super::perf::C::WaitRoomPresentNs
        } else {
            super::perf::C::WaitRoomNs
        },
        t0.elapsed().as_nanos() as u64,
    );
    if present {
        super::occl::PRESENT_WAIT_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
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
    if chain_trace::on() {
        let (k, oq) = match &op {
            Op::Submit { q, .. } => (1, *q),
            Op::Present { q, .. } => (2, *q),
            Op::Call(_) => (3, 0),
            Op::Retry => (4, 0),
            Op::WaitReceipt { q, .. } => (5, *q),
        };
        chain_trace::note("push", seq, k << 32 | (oq & 0xffff_ffff));
    }
    before(seq);
    q.push_back((seq, op));
    drop(q);
    a.cv.notify_one();
    seq
}

/// Ops queued and not yet replayed.
pub fn aq_busy() -> bool {
    AQ.get()
        .is_some_and(|a| a.done.load(Ordering::Acquire) < a.tail.load(Ordering::Acquire))
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
        l = a
            .idle_cv
            .wait_timeout(l, std::time::Duration::from_millis(20))
            .unwrap()
            .0;
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
        l = a
            .idle_cv
            .wait_timeout(l, std::time::Duration::from_millis(2))
            .unwrap()
            .0;
    }
}

/// The sequence number of the last op queued (0: none). Read before a pool scan: the game's CPU writes before it
/// queued any op up to this one happened before the read (`mem::scan_covers`).
pub fn aq_tail() -> u64 {
    AQ.get().map(|a| a.tail.load(Ordering::Acquire)).unwrap_or(0)
}

/// The newest op the replay thread has taken (set before the op runs).
static AQ_TAKEN: AtomicU64 = AtomicU64::new(0);
/// Parked work that ran on (a sync of its own), and submits that parked at their first record (no submission made).
pub static RESUMES: AtomicU64 = AtomicU64::new(0);
pub static PARKED_AT_ONCE: AtomicU64 = AtomicU64::new(0);

/// The replay op a parked queue's rest counts as for its pool sync (`mem::scan_covers`), as it runs on now. What it
/// may read was written by the CPU before the game queued the parked work, or before the game queued the op whose GPU
/// signal or receipt released it (both replayed, so at most the newest op taken), or before a CPU event signal (which
/// `scan_covers` refuses on its own). Every such op is at most `AQ_TAKEN`; 0 (no replay thread): it scans.
/// `NXGFX_WALKER_PARKED=0`: a present behind parked work is not covered by the pool walker's walk.
fn walker_parked() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_WALKER_PARKED").as_deref() != Ok("0"))
}

fn resume_due() -> Option<u64> {
    RESUMES.fetch_add(1, Ordering::Relaxed);
    Some(AQ_TAKEN.load(Ordering::Acquire))
}

/// Ops queued and not replayed yet.
pub fn aq_backlog() -> u64 {
    AQ.get()
        .map(|a| a.tail.load(Ordering::Acquire) - a.done.load(Ordering::Acquire))
        .unwrap_or(0)
}

/// Replay time on the replay thread (ns, cumulative).
pub fn aq_ns() -> u64 {
    AQ.get().map(|a| a.ns.load(Ordering::Relaxed)).unwrap_or(0)
}

fn aq_main(g: &'static Gpu, a: &'static Aq) {
    super::rtclass::join("vk replay");
    loop {
        super::rtclass::tick();
        // the op, and the sequence numbers of the submits of the same queue queued right behind it and taken with it
        let mut merged: Vec<u64> = Vec::new();
        let t_idle = std::time::Instant::now();
        let (mut seq, mut op) = {
            let mut q = a.ops.lock().unwrap();
            loop {
                if let Some(x) = q.pop_front() {
                    break x;
                }
                q = a.cv.wait(q).unwrap();
            }
        };
        let t_busy = std::time::Instant::now();
        super::perf::add(
            super::perf::C::AqIdleNs,
            (t_busy - t_idle).as_nanos() as u64,
        );
        // submits of one queue already waiting one after another replay as one submission: one pool sync (a write-
        // watch scan of every mirror: Mario Kart World's ~3,500 pools cost ~10 ms a submission) and one queue submit
        // for all of them. Their fences all resolve to the merged submission's (later for the first: conservative).
        // NXGFX_AQ_MERGE=0: one submission each.
        if let Op::Submit {
            q: oq,
            stream: ref mut s0,
            ..
        } = op
        {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *ON.get_or_init(|| std::env::var("NXGFX_AQ_MERGE").as_deref() != Ok("0")) {
                let mut q = a.ops.lock().unwrap();
                while merged.len() < 7 && s0.len() < 64 << 20 {
                    match q.front() {
                        Some((_, Op::Submit { q: nq, .. })) if *nq == oq => {}
                        _ => break,
                    }
                    let Some((ns, Op::Submit { stream, .. })) = q.pop_front() else {
                        break;
                    };
                    merged.push(seq);
                    seq = ns;
                    s0.extend_from_slice(&stream);
                    AQ_MERGED.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        // before it runs: a parked queue that runs on from now was released by this op or an earlier one
        AQ_TAKEN.fetch_max(seq, Ordering::AcqRel);
        let t0 = std::time::Instant::now();
        // the NVN queue of a submit or present: its fence is a receipt while that queue stays parked
        let opq = match &op {
            Op::Submit { q, .. } | Op::Present { q, .. } | Op::WaitReceipt { q, .. } => Some(*q),
            _ => None,
        };
        let kind = match &op {
            Op::Submit { .. } => "submit",
            Op::Present { .. } => "present",
            Op::Call(_) => "callback",
            Op::Retry => "retry",
            Op::WaitReceipt { .. } => "wait receipt",
        };
        wait_diag::mark(0, kind, [opq.unwrap_or(0), seq]);
        chain_trace::note("op", seq, merged.len() as u64);
        match op {
            Op::Submit { q, ll, stream } => {
                let mut st = g.q.sub.lock().unwrap();
                take_recycled(g, &mut st);
                if let Some(p) = st.parked.get_mut(&q) {
                    p.stream.extend_from_slice(&stream);
                } else {
                    run_stream_due(g, &mut st, q, ll, stream, Vec::new(), Some(seq));
                }
            }
            Op::Present { q, ll, t, crop } => {
                present_now(g, q, ll, &t, crop, Some(seq));
                AQ_PRESENTS.fetch_sub(1, Ordering::AcqRel);
            }
            Op::Call(f) => f(g),
            Op::Retry => retry_parked(g),
            Op::WaitReceipt { q, r } => {
                // an async fence: the receipt its op left when its queue was parked (else the timeline orders it)
                let was_async = r & RECEIPT == 0;
                let r = if was_async {
                    a.fences
                        .lock()
                        .unwrap()
                        .range((r & !ASYNC)..)
                        .next()
                        .map(|(_, v)| *v)
                        .unwrap_or(0)
                } else {
                    r
                };
                let mut st = g.q.sub.lock().unwrap();
                if r & RECEIPT != 0 && !g.q.receipts.lock().unwrap().contains_key(&r) {
                    if was_async {
                        if !ASYNC_WAIT_LOGGED.swap(true, Ordering::Relaxed) {
                            crate::vklog!("a queue waits on another queue's parked work (an async fence of a submit made while that queue waited on an event)");
                        }
                    }
                    park_on_receipt(&mut st, q, r);
                }
            }
        }
        // parked queues whose wait now passes run on (a receipt resolved, an event signalled)
        wait_diag::mark(0, "retry parked", [opq.unwrap_or(0), seq]);
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
        wait_diag::mark(0, "publish async fence/wake", [opq.unwrap_or(0), seq]);
        chain_trace::note("op_done", seq, fence);
        a.done.store(seq, Ordering::Release);
        {
            let _l = a.idle_mu.lock().unwrap();
            a.idle_cv.notify_all();
        }
        // fence waiters look again
        let _d = g.q.done.lock().unwrap();
        g.q.done_cv.notify_all();
        drop(_d);
        super::perf::add(
            super::perf::C::AqBusyNs,
            t_busy.elapsed().as_nanos() as u64,
        );
        wait_diag::mark(0, "idle", [0, seq]);
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
        let mut occl = super::occl::Walk::default();
        unsafe {
            crate::nvn::cmd::for_each_record(handles, |op, r| {
                if op == crate::nvn::obj::op::FENCE_SYNC && r.len() >= 16 {
                    syncs.push(core::ptr::read_unaligned(r.as_ptr().add(8) as *const u64));
                }
                occl.record(op, r);
                stream.extend_from_slice(r)
            })
        };
        super::occl::submitted(&occl);
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
        // still queued and the game rewrote memory that work reads (NXGFX_ASYNC=0 replays at
        // once). Written before the replay thread can see the op: its exact value (set when the replay reaches the
        // record, before a later event wait parks the queue) must not be overwritten by this one (that waited for the
        // parked rest: Donkey Kong Bananza's main thread held 15 s in nvnSyncWait)
        let seq = aq_push_with(
            g,
            Op::Submit {
                q: qserial,
                ll: origin_ll,
                stream,
            },
            |seq| {
                for &s in syncs.iter().filter(|&&s| s != 0) {
                    let so = s as *mut crate::nvn::obj::sync_t;
                    unsafe {
                        core::ptr::write_volatile(&mut (*so).fence, ASYNC | seq);
                        core::ptr::write_volatile(&mut (*so).fenced, 1);
                    }
                }
            },
        );
        LAST_SEQ
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(qserial, seq);
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
        aq_push(
            g,
            Op::Submit {
                q: qserial,
                ll: origin_ll,
                stream,
            },
        );
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
fn run_stream(
    g: &'static Gpu,
    st: &mut SubmitState,
    qserial: u64,
    origin_ll: bool,
    stream: Vec<u8>,
    receipts: Vec<u64>,
) {
    run_stream_due(g, st, qserial, origin_ll, stream, receipts, None)
}

/// `run_stream` for replay op `due` (its pool sync may be covered by an earlier scan: `mem::scan_covers`).
fn run_stream_due(
    g: &'static Gpu,
    st: &mut SubmitState,
    qserial: u64,
    origin_ll: bool,
    stream: Vec<u8>,
    receipts: Vec<u64>,
    due: Option<u64>,
) {
    // a stream that starts at a wait not passing parks before any work: no submission and no pool sync for it (the
    // sync its work needs is made when it runs on)
    if receipts.is_empty() && !exec::wait_passes(g, &stream) {
        PARKED_AT_ONCE.fetch_add(1, Ordering::Relaxed);
        // the queue's origin is read from its state when it runs on
        st.qstates
            .entry(qserial)
            .or_insert_with(|| QState::new(origin_ll));
        st.parked.insert(
            qserial,
            Parked {
                stream,
                receipts,
                wait_receipt: None,
                presents: Vec::new(),
                later: Vec::new(),
            },
        );
        request_retry(g);
        return;
    }
    wait_diag::mark(1, "new submission", [qserial, 0]);
    let mut sub = new_sub(g, st);
    wait_diag::mark(1, "prepare pools/textures", [qserial, sub.serial]);
    // a changed resolution scale makes the scaled render targets again first
    super::scale::begin_submit(g, &mut sub);
    let t0 = std::time::Instant::now();
    let mut nd = 0;
    let mut tp = 0.0;
    // the CPU signals made before this submission's pool sync (`chain_parked`)
    let sig0 = super::mem::cpu_signals_now();
    super::perf::timed(super::perf::C::PrepNs, || {
        super::mem::SYNC_BY[0].fetch_add(1, Ordering::Relaxed);
        let dirty = super::perf::timed(super::perf::C::PoolSyncNs, || {
            super::mem::begin_submit(g, &mut sub, due)
        });
        tp = t0.elapsed().as_secs_f64();
        nd = dirty.len();
        super::gmem::begin(g, &mut sub);
        g.image_begin_submit(&mut sub, &dirty);
    });
    let tt = t0.elapsed().as_secs_f64();
    chain_trace::note("prep", (tp * 1e6) as u64, (tt * 1e6) as u64);
    if tt > 1.0 {
        crate::vklog!("a submission's set-up took {tt:.2} s: pools {tp:.2} s ({nd} CPU-written ranges), textures {:.2} s", tt - tp);
    }
    let mut qs = st
        .qstates
        .remove(&qserial)
        .unwrap_or_else(|| QState::new(origin_ll));
    let segs0 = exec::seg_stats::SEGMENTS.load(Ordering::Relaxed);
    let rest = super::perf::timed(super::perf::C::ReplayNs, || {
        // a long stream in segments (`NXGFX_TRANSLATE_THREADS`), each in a child submission of this one
        exec::run_segmented(g, &mut sub, &mut qs, &stream, &mut || new_sub(g, st))
            .unwrap_or_else(|| exec::run(g, &mut sub, &mut qs, &stream))
    });
    wait_diag::phase("end replay/pass");
    exec::end(g, &mut sub, &mut qs);
    note_xlated(stream.len(), segs0);
    st.qstates.insert(qserial, qs);
    if park_chain() {
        // (MKW-PIPE) the rest parks first; then the parked queues the GPU signals recorded so far release run on
        let mut done = Vec::new();
        let mut reparked = false;
        match rest {
            Some(off) => {
                note_park(&stream[off..]);
                st.parked.insert(
                    qserial,
                    Parked {
                        stream: park_rest(stream, off),
                        receipts,
                        wait_receipt: None,
                        presents: Vec::new(),
                        later: Vec::new(),
                    },
                );
                reparked = true;
            }
            None => done = receipts,
        }
        reparked |= chain_parked(g, st, &mut sub, sig0, &mut done);
        let value = submit_sub(g, sub);
        wait_diag::phase("submitted");
        resolve_receipts(g, &mut done, value);
        if reparked && !st.parked.is_empty() {
            request_retry(g);
        }
        return;
    }
    let value = submit_sub(g, sub);
    wait_diag::phase("submitted");
    match rest {
        // parked again: its receipts wait for the rest
        Some(off) => {
            note_park(&stream[off..]);
            st.parked.insert(
                qserial,
                Parked {
                    stream: park_rest(stream, off),
                    receipts,
                    wait_receipt: None,
                    presents: Vec::new(),
                    later: Vec::new(),
                },
            );
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

/// (MKW-AHEAD) A stream translated: its bytes and the segments its translation was cut into (0: one piece).
fn note_xlated(len: usize, segs0: u64) {
    if chain_trace::on() {
        let n = exec::seg_stats::SEGMENTS.load(Ordering::Relaxed).wrapping_sub(segs0);
        chain_trace::note("xlated", len as u64, n);
    }
}

/// (MKW-SYNC) The records of `stream` from `off` on, for a queue that parks there: the stream's own buffer without
/// its head (a copy into a new buffer was a fresh allocation of up to ~1 MB, its pages faulted in, a few times a frame
/// in Mario Kart World: ~0.5 ms of the replay thread a frame). `NXGFX_PARK_IN_PLACE=0`: the copy, as before.
fn park_rest(mut stream: Vec<u8>, off: usize) -> Vec<u8> {
    static COPY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *COPY.get_or_init(|| std::env::var("NXGFX_PARK_IN_PLACE").as_deref() == Ok("0")) {
        return stream[off..].to_vec();
    }
    stream.drain(..off.min(stream.len()));
    stream
}

fn note_park(rest: &[u8]) {
    if chain_trace::on() {
        let (a, v) = exec::wait_of(rest);
        chain_trace::note("park", a, v);
    }
}

/// The receipts of streams that ran to their end in the submission of timeline `value`.
fn resolve_receipts(g: &Gpu, done: &mut Vec<u64>, value: u64) {
    if done.is_empty() {
        return;
    }
    let mut r = g.q.receipts.lock().unwrap();
    for id in done.drain(..) {
        r.insert(id, value);
    }
    drop(r);
    g.q.done_cv.notify_all();
}

/// (MKW-PIPE) `NXGFX_PARK_CHAIN` (default on; 0: off): when a stream parks at an event wait another queue's work
/// will signal, the parked queues whose wait a GPU signal recorded so far passes run on at once, on the replay thread,
/// each in a submission of its own made without a pool sync (`chained_sub`), until none passes. The two NVN queues of
/// Mario Kart World's frame hand work to each other this way about nine times a frame; each hand-over went through
/// the parked-work retry and a full pool sync (a whole write-watch scan whenever the game had made a CPU signal
/// meanwhile, which it does every frame on other events), in series on the replay thread, which the game's frame
/// waits for (its main thread's nvnSyncWait on the frame's fence). The first sync covers them all: every parked
/// stream was queued before the op this submission replays, and its wait passes by GPU work recorded after that
/// sync, not by a CPU signal (`exec::wait_passes_chained`). CPU writes made since stay in the write watch for the
/// next ordinary sync, as they would for a GPU that had not reached them yet. Each keeps a submission of its own so
/// the GPU works on one while the next is translated (one submission for all of them left the frame's GPU work to
/// after its last translation: 10.6 ms from the end of the replay to the game's fence, against 1.0).
fn park_chain() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("NXGFX_PARK_CHAIN").as_deref() != Ok("0");
        crate::vklog!(
            "parked queues released by GPU signals already recorded: {}",
            if on {
                "run on at once, without a pool sync of their own (NXGFX_PARK_CHAIN=0: through the retry, each with its own)"
            } else {
                "through the retry, each with its own pool sync (NXGFX_PARK_CHAIN=0)"
            }
        );
        on
    })
}

/// Streams run on after one pool sync at most.
const CHAIN_MAX: usize = 32;

/// A submission for a chained stream (`chain_parked`): as `run_stream_due`'s, without the pool sync (the textures'
/// own preparation still runs: registrations, landed texture write-backs, images due).
fn chained_sub(g: &'static Gpu, st: &mut SubmitState) -> Sub {
    let mut sub = new_sub(g, st);
    super::scale::begin_submit(g, &mut sub);
    super::perf::timed(super::perf::C::PrepNs, || {
        super::gmem::begin(g, &mut sub);
        g.image_begin_submit(&mut sub, &[]);
    });
    chain_trace::note("prep", 0, 0);
    sub
}

/// Runs on the parked queues whose wait now passes by GPU work (`exec::wait_passes_chained`), each in a submission
/// of its own (`chained_sub`): `sub` (recorded so far) is submitted before each and replaced; `sig0`: the CPU signals
/// made before the last pool sync. The receipts of the streams that ran to their end go to `done` (resolved with the
/// value of the submission that holds them). Returns whether one parked again.
fn chain_parked(
    g: &'static Gpu,
    st: &mut SubmitState,
    sub: &mut Sub,
    sig0: u64,
    done: &mut Vec<u64>,
) -> bool {
    let mut reparked = false;
    for _ in 0..CHAIN_MAX {
        let quiet = super::mem::cpu_signals_now() == sig0;
        let pick = st
            .parked
            .iter()
            .filter(|(_, p)| {
                p.wait_receipt.is_none()
                    && p.presents.is_empty()
                    && p.later.is_empty()
                    && exec::wait_passes_chained(g, &p.stream, quiet)
            })
            .map(|(k, _)| *k)
            .min();
        let Some(q) = pick else { break };
        let p = st.parked.remove(&q).unwrap();
        chain_trace::note("chain", exec::wait_of(&p.stream).0, q);
        super::perf::inc(super::perf::C::Chained);
        let prev = std::mem::replace(sub, chained_sub(g, st));
        let value = submit_sub(g, prev);
        resolve_receipts(g, done, value);
        let ll = st.qstates.get(&q).map(|s| s.origin_ll).unwrap_or(true);
        let mut qs = st.qstates.remove(&q).unwrap_or_else(|| QState::new(ll));
        let stream = p.stream;
        let segs0 = exec::seg_stats::SEGMENTS.load(Ordering::Relaxed);
        let rest = super::perf::timed(super::perf::C::ReplayNs, || {
            exec::run_segmented(g, sub, &mut qs, &stream, &mut || new_sub(g, st))
                .unwrap_or_else(|| exec::run(g, sub, &mut qs, &stream))
        });
        exec::end(g, sub, &mut qs);
        note_xlated(stream.len(), segs0);
        st.qstates.insert(q, qs);
        match rest {
            Some(off) => {
                note_park(&stream[off..]);
                st.parked.insert(
                    q,
                    Parked {
                        stream: park_rest(stream, off),
                        receipts: p.receipts,
                        wait_receipt: None,
                        presents: Vec::new(),
                        later: Vec::new(),
                    },
                );
                reparked = true;
            }
            None => done.extend(p.receipts),
        }
    }
    reparked
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
            let what = if p.wait_receipt.is_some() && p.stream.is_empty() {
                "a sync of another parked queue".to_string()
            } else {
                exec::wait_desc(&p.stream)
            };
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
    let passes = |p: &Parked| {
        p.wait_receipt
            .is_none_or(|r| g.q.receipts.lock().unwrap().contains_key(&r))
            && (p.stream.is_empty() || exec::wait_passes(g, &p.stream))
    };
    loop {
        let ready: Vec<u64> = st
            .parked
            .iter()
            .filter(|(_, p)| passes(p))
            .map(|(k, _)| *k)
            .collect();
        if ready.is_empty() {
            return;
        }
        for q in ready {
            // (OOT-COMBO) running an earlier ready queue may have run this one on already (`chain_parked`, MKW-PIPE's
            // park chain, inside `run_stream_due`) and possibly parked it again at a later wait: take it only while
            // it is still parked and its wait still passes (Ocarina of Time's forest load: the replay thread
            // panicked on the missing entry and the game hung)
            if !st.parked.get(&q).is_some_and(passes) {
                continue;
            }
            let mut p = st.parked.remove(&q).unwrap();
            if chain_trace::on() {
                chain_trace::note("resume", exec::wait_of(&p.stream).0, q);
            }
            let ll = st.qstates.get(&q).map(|s| s.origin_ll).unwrap_or(true);
            if let Some(&(off, r)) = p.later.first() {
                // the work up to a queued wait runs; the rest parks behind that wait (its receipts and presents last)
                let tail = p.stream.split_off(off.min(p.stream.len()));
                let later: Vec<(usize, u64)> =
                    p.later[1..].iter().map(|&(o, x)| (o - off, x)).collect();
                if !p.stream.is_empty() {
                    run_stream_due(g, st, q, ll, p.stream, Vec::new(), resume_due());
                }
                match st.parked.get_mut(&q) {
                    Some(again) => {
                        let base = again.stream.len();
                        again.later.push((base, r));
                        again
                            .later
                            .extend(later.into_iter().map(|(o, x)| (o + base, x)));
                        again.stream.extend_from_slice(&tail);
                        again.receipts.extend(p.receipts);
                        again.presents.extend(p.presents);
                    }
                    None => {
                        st.parked.insert(
                            q,
                            Parked {
                                stream: tail,
                                receipts: p.receipts,
                                wait_receipt: Some(r),
                                presents: p.presents,
                                later,
                            },
                        );
                    }
                }
                continue;
            }
            if !p.stream.is_empty() || !p.receipts.is_empty() {
                run_stream_due(g, st, q, ll, p.stream, p.receipts, resume_due());
            }
            match st.parked.get_mut(&q) {
                // parked again: its presents stay behind
                Some(again) => again.presents.extend(p.presents),
                None => {
                    for (t, crop) in p.presents {
                        let frame = present_frame(&t, crop, ll, false);
                        // a present behind parked work: the pool walker's walk covers its sync as a direct one's when
                        // it started after every op taken so far (`resume_due`, at or after this present's queuing);
                        // the parked work ran first with syncs of its own. Without it this sync walked every pool,
                        // cold ones too (4-9 ms in Mario Kart World, whose presents often wait behind parked work).
                        // `NXGFX_WALKER_PARKED=0`: the strict rule as before.
                        let direct = walker_parked();
                        present_locked(g, st, q, ll, &t, crop, frame, resume_due(), direct);
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
        // the queue's own last op (another queue's parked work is not waited for); 0: every queue's
        let t = a.tail.load(Ordering::Acquire);
        let s = if qserial != 0 {
            LAST_SEQ
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|m| m.get(&qserial).copied())
                .unwrap_or(t)
        } else {
            t
        };
        if !on_replay_thread()
            && (a.done.load(Ordering::Acquire) < s || aq_physical(ASYNC | s).is_none())
        {
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
        if fence & ASYNC != 0
            && AQ.get().is_some()
            && !on_replay_thread()
            && aq_physical(fence).is_none()
        {
            aq_push(
                g,
                Op::WaitReceipt {
                    q: _qserial,
                    r: fence,
                },
            );
        }
        return;
    }
    if g.q.receipts.lock().unwrap().contains_key(&fence) {
        return;
    }
    // the waiting queue's later work runs once that sync's submission has (it parks behind it)
    if aq_on() {
        aq_push(
            g,
            Op::WaitReceipt {
                q: _qserial,
                r: fence,
            },
        );
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
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_nanos(timeout_ns.min(3600_000_000_000));
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
        d =
            g.q.done_cv
                .wait_timeout(
                    d,
                    (deadline - now).min(std::time::Duration::from_millis(50)),
                )
                .unwrap()
                .0;
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
pub fn present(
    qserial: u64,
    origin_ll: bool,
    t: &crate::nvn::obj::tex_t,
    crop: Option<[i32; 4]>,
) -> u64 {
    let Some(g) = Gpu::try_get() else { return 0 };
    super::capture::present(qserial, origin_ll, t, crop);
    if aq_on() {
        // the pool walker starts the frame's walk of every pool now, before the replay thread can take the present
        return ASYNC
            | aq_push_with(
                g,
                Op::Present {
                    q: qserial,
                    ll: origin_ll,
                    t: *t,
                    crop,
                },
                |_| super::mem::walker_kick(),
            );
    }
    present_now(g, qserial, origin_ll, t, crop, None)
}

fn present_now(
    g: &'static Gpu,
    qserial: u64,
    origin_ll: bool,
    t: &crate::nvn::obj::tex_t,
    crop: Option<[i32; 4]>,
    due: Option<u64>,
) -> u64 {
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
    present_locked(g, &mut st, qserial, origin_ll, t, crop, frame, due, due.is_some())
}

/// A present's frame number, telemetry and pass-dump bookkeeping, and the screenshot requested for it (`shot`:
/// false where the submission state is held).
fn present_frame(
    t: &crate::nvn::obj::tex_t,
    crop: Option<[i32; 4]>,
    origin_ll: bool,
    shot: bool,
) -> u64 {
    let frame = super::present::note_present();
    super::perf::frame(frame);
    super::exec::op_times::frame(frame);
    super::exec::seg_stats::frame(frame);
    super::scale::frame();
    super::exec::passdump::frame(frame, crate::log::elapsed());
    if shot {
        super::shot::on_present(t, frame, crop, origin_ll);
    }
    frame
}

/// A present in queue order (the submission state locked).
fn present_locked(
    g: &'static Gpu,
    st: &mut SubmitState,
    _qserial: u64,
    origin_ll: bool,
    t: &crate::nvn::obj::tex_t,
    crop: Option<[i32; 4]>,
    _frame: u64,
    due: Option<u64>,
    // taken straight from the replay queue (not behind parked work): the pool walker's walk may cover its sync
    direct: bool,
) -> u64 {
    wait_diag::mark(1, "present: new submission", [_qserial, 0]);
    let mut sub = new_sub(g, st);
    wait_diag::mark(1, "present: prepare pools/textures", [_qserial, sub.serial]);
    // a GPU-drawn picture presented: nothing of guest memory reaches this submission but its image (prepared by
    // `record_present` itself), so the pool sync and the image walk are left to the next submission
    let lean = present_lean() && {
        let imgs = g.images.lock().unwrap();
        imgs.by_serial.get(&t.serial).is_some_and(|im| super::image::presented_gpu_drawn(im))
    };
    if lean {
        super::perf::inc(super::perf::C::PresentsLean);
    } else {
    super::mem::SYNC_BY[1].fetch_add(1, Ordering::Relaxed);
    super::perf::timed(super::perf::C::PrepNs, || {
        let dirty = super::perf::timed(super::perf::C::PoolSyncNs, || {
            super::mem::begin_submit_ex(g, &mut sub, due, direct)
        });
        g.image_begin_submit(&mut sub, &dirty);
    });
    }
    wait_diag::phase("present: record");
    super::present::record_present(g, &mut sub, t, origin_ll, crop);
    super::shot::record_sink(g, &mut sub, t, crop, origin_ll);
    submit_sub(g, sub)
}

/// `NXGFX_PRESENT_LEAN` (default on; `0`: every present syncs the pools and walks the images, as before): a present
/// of a GPU-drawn picture (`image::presented_gpu_drawn`) leaves the pool sync and the image walk to the next
/// submission, which makes them before any GPU work could read guest memory (MKW-SETS: the present's sync and walk
/// were ~14 % of the replay thread's busy time in MKW's race).
fn present_lean() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_PRESENT_LEAN").as_deref() != Ok("0"))
}

pub fn present_wait(ticket: u64) {
    wait_fence(ticket, u64::MAX);
}

/// `NXGFX_LAND_FIRST` (default on; `0`: a sync takes the pool lock at once, as before): before a pool sync on the
/// replay thread takes the pool lock, submissions the GPU has finished but the completion thread has not landed yet
/// land first (at most `LAND_FIRST_MAX`). MKW-HEAVY: a sync of the next frame's early work (the second queue's submit,
/// replayed while the frame's fence was landing) held the pool lock 0.9 ms while the landing of the frame's last
/// submissions waited for it, and the game waited for that fence: the frame's tail. Returns the ns waited.
pub fn land_first(g: &Gpu) -> u64 {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_LAND_FIRST").as_deref() != Ok("0")) || !on_replay_thread() {
        return 0;
    }
    let Ok(gpu) = (unsafe { g.dev().get_semaphore_counter_value(g.q.timeline) }) else {
        return 0;
    };
    if gpu <= g.q.completed() {
        return 0;
    }
    let t = std::time::Instant::now();
    wait_fence(gpu, LAND_FIRST_MAX);
    let ns = t.elapsed().as_nanos() as u64;
    chain_trace::note("land_first", gpu, ns / 1000);
    ns
}

/// The longest a sync waits for landings (`land_first`): 1.5 ms.
const LAND_FIRST_MAX: u64 = 1_500_000;

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
    wait_diag::mark(1, "oneoff: new submission", [0, 0]);
    let mut sub = new_sub(g, &mut st);
    wait_diag::mark(1, "oneoff: prepare pools/textures", [0, sub.serial]);
    super::mem::SYNC_BY[2].fetch_add(1, Ordering::Relaxed);
    super::perf::timed(super::perf::C::PrepNs, || {
        let dirty = super::perf::timed(super::perf::C::PoolSyncNs, || {
            super::mem::begin_submit(g, &mut sub, None)
        });
        g.image_begin_submit(&mut sub, &dirty);
    });
    {
        wait_diag::phase("oneoff: images/record");
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

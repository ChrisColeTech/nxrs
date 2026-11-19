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
    pub post: Vec<Post>,
    pub query: vk::QueryPool,
    pub queries: u32,
    pub images_new: Vec<vk::Image>,
    pub transient: Vec<Buf>,
    pub transient_views: Vec<vk::ImageView>,
    pub present: Option<super::present::PendingPresent>,
}
unsafe impl Send for Sub {}

pub enum Post {
    Event { addr: u64, mode: u32, value: u32 },
    Counter { addr: u64, kind: u32, query: u32 },
    Readback { dst: *mut u8, src: *const u8, n: usize },
    Callback(Box<dyn FnOnce(&Gpu) + Send>),
}
unsafe impl Send for Post {}

struct Inflight {
    value: u64,
    sub: Sub,
    wb: Vec<(u64, u64, u64, *const u8)>,
    readback: Option<Buf>,
}
unsafe impl Send for Inflight {}

struct Parked {
    /// canonical records still to run, owned
    stream: Vec<u8>,
    receipts: Vec<u64>,
}

pub struct SubmitState {
    pub qstates: HashMap<u64, QState>,
    parked: HashMap<u64, Parked>,
    cmd_pool: vk::CommandPool,
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
        Ok(Queue {
            timeline,
            sub: Mutex::new(SubmitState { qstates: HashMap::new(), parked: HashMap::new(), cmd_pool, free_subs: Vec::new() }),
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
            receipts: Mutex::new(HashMap::new()),
            next_receipt: AtomicU64::new(1),
            thread_started: AtomicBool::new(false),
        })
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
}

impl Sub {
    /// Copies `src` into staging memory; returns (buffer, offset). Aligned to 256 (usable as a uniform buffer).
    pub fn stage_bytes(&mut self, g: &Gpu, src: &[u8]) -> (vk::Buffer, u64) {
        let n = src.len() as u64;
        let (b, off) = self.stage_alloc(g, n);
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), self.stage[b].ptr.add(off as usize), src.len()) };
        (self.stage[b].buf, off)
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
        let b = g
            .buffer(
                size,
                vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::CpuToGpu,
                "staging",
            )
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
        let ai = vk::CommandBufferAllocateInfo::default().command_pool(st.cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(2);
        let cbs = unsafe { g.dev().allocate_command_buffers(&ai) }.expect("command buffers");
        let qi = vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(256);
        let query = unsafe { g.dev().create_query_pool(&qi, None) }.expect("query pool");
        Sub {
            pre: cbs[0],
            main: cbs[1],
            stage: Vec::new(),
            stage_used: 0,
            dpools: Vec::new(),
            dpool_i: 0,
            writebacks: Vec::new(),
            post: Vec::new(),
            query,
            queries: 0,
            images_new: Vec::new(),
            transient: Vec::new(),
            transient_views: Vec::new(),
            present: None,
        }
    };
    let bi = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe {
        g.dev().reset_command_buffer(s.pre, vk::CommandBufferResetFlags::empty()).unwrap();
        g.dev().reset_command_buffer(s.main, vk::CommandBufferResetFlags::empty()).unwrap();
        g.dev().begin_command_buffer(s.pre, &bi).unwrap();
        g.dev().begin_command_buffer(s.main, &bi).unwrap();
        g.dev().cmd_reset_query_pool(s.pre, s.query, 0, 256);
    }
    barrier_all(g, s.main);
    s.stage_used = 0;
    // keep one staging chunk; the rest go
    while s.stage.len() > 1 {
        let b = s.stage.pop().unwrap();
        g.free_buffer(b);
    }
    s
}

/// Ends and submits a recorded submission; returns its timeline value.
fn submit_sub(g: &'static Gpu, mut s: Sub) -> u64 {
    let q = &g.q;
    // write-backs: copy the GPU-written ranges into one readback buffer at the end of the replay
    let total: u64 = s.writebacks.iter().map(|w| (w.2 + 15) & !15).sum();
    let mut readback = None;
    let mut wb = Vec::new();
    if total > 0 {
        barrier_all(g, s.main);
        let rb = g.buffer(total, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "write-back").expect("readback");
        let mut off = 0;
        for &(pool, a, n, buf, boff) in &s.writebacks {
            let r = [vk::BufferCopy { src_offset: boff, dst_offset: off, size: n }];
            unsafe { g.dev().cmd_copy_buffer(s.main, buf, rb.buf, &r) };
            wb.push((pool, a, n, unsafe { rb.ptr.add(off as usize) } as *const u8));
            off += (n + 15) & !15;
        }
        readback = Some(rb);
    }
    // host reads of anything this submission wrote
    let mb = [vk::MemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::HOST)
        .dst_access_mask(vk::AccessFlags2::HOST_READ)];
    unsafe { g.dev().cmd_pipeline_barrier2(s.main, &vk::DependencyInfo::default().memory_barriers(&mb)) };
    barrier_all(g, s.pre);
    unsafe {
        g.dev().end_command_buffer(s.pre).unwrap();
        g.dev().end_command_buffer(s.main).unwrap();
    }
    let _vq = q.vkq.lock().unwrap();
    let value = q.last.load(Ordering::Acquire) + 1;
    let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(s.pre), vk::CommandBufferSubmitInfo::default().command_buffer(s.main)];
    let mut sig = vec![vk::SemaphoreSubmitInfo::default().semaphore(q.timeline).value(value).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    let mut wait = vec![];
    if let Some(p) = &s.present {
        wait.push(vk::SemaphoreSubmitInfo::default().semaphore(p.acquired).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS));
        sig.push(vk::SemaphoreSubmitInfo::default().semaphore(p.rendered).stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS));
    }
    let si = [vk::SubmitInfo2::default().command_buffer_infos(&cbs).signal_semaphore_infos(&sig).wait_semaphore_infos(&wait)];
    unsafe { g.dev().queue_submit2(g.d.queue, &si, vk::Fence::null()) }.unwrap_or_else(|e| panic!("vkQueueSubmit2: {e}"));
    q.last.store(value, Ordering::Release);
    if let Some(p) = s.present.take() {
        super::present::queue_present(g, p);
    }
    drop(_vq);
    start_completion(g);
    q.inflight.lock().unwrap().push_back(Inflight { value, sub: s, wb, readback });
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
                Err(e) => panic!("vkWaitSemaphores: {e}"),
            }
        }
        let mut signalled = false;
        // CPU-visible effects, in order: write-backs first (an event may tell the game to read them)
        super::mem::apply_writebacks(g, &item.wb);
        let mut sub = item.sub;
        let ts = if sub.queries > 0 {
            let mut v = vec![0u64; sub.queries as usize];
            unsafe { g.dev().get_query_pool_results(sub.query, 0, &mut v, vk::QueryResultFlags::TYPE_64) }.ok();
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
                    let ticks = ts.get(query as usize).copied().unwrap_or(0);
                    let ns = (ticks as f64 * g.timestamp_period() as f64) as u64;
                    let t = crate::nvn::device::timestamp_from_ns(crate::nvn::device::now_ns().min(ns.max(1)).max(ns));
                    let counter = if kind == crate::nvn::types::counter::TIMESTAMP || kind == crate::nvn::types::counter::TIMESTAMP_TOP { t } else { 0 };
                    unsafe { core::ptr::write_unaligned(addr as *mut [u64; 2], [counter, t]) };
                }
                Post::Readback { dst, src, n } => unsafe { core::ptr::copy_nonoverlapping(src, dst, n) },
                Post::Callback(f) => f(g),
            }
        }
        if let Some(rb) = item.readback {
            g.free_buffer(rb);
        }
        for b in sub.transient.drain(..) {
            g.free_buffer(b);
        }
        for v in sub.transient_views.drain(..) {
            unsafe { g.dev().destroy_image_view(v, None) };
        }
        for p in &sub.dpools {
            unsafe { g.dev().reset_descriptor_pool(*p, vk::DescriptorPoolResetFlags::empty()) }.ok();
        }
        sub.dpool_i = 0;
        sub.writebacks.clear();
        sub.queries = 0;
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
                        f = g.q.retry_cv.wait(f).unwrap();
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

/// nvnQueueSubmitCommands.
pub fn submit(qserial: u64, origin_ll: bool, handles: &[u64]) {
    let Some(g) = Gpu::try_get() else { return };
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

/// Replays a stream for a queue in one submission; parks the queue at an unsatisfied wait.
fn run_stream(g: &'static Gpu, st: &mut SubmitState, qserial: u64, origin_ll: bool, stream: Vec<u8>, receipts: Vec<u64>) {
    let mut sub = new_sub(g, st);
    let dirty = super::mem::begin_submit(g, &mut sub);
    g.image_begin_submit(&mut sub, &dirty);
    let mut qs = st.qstates.remove(&qserial).unwrap_or_else(|| QState::new(origin_ll));
    let rest = exec::run(g, &mut sub, &mut qs, &stream);
    exec::end(g, &mut sub, &mut qs);
    st.qstates.insert(qserial, qs);
    let value = submit_sub(g, sub);
    match rest {
        // parked again: its receipts wait for the rest
        Some(off) => {
            st.parked.insert(qserial, Parked { stream: stream[off..].to_vec(), receipts });
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

/// Parked queues whose wait now passes run on.
pub fn retry_parked(g: &'static Gpu) {
    let mut st = g.q.sub.lock().unwrap();
    retry_locked(g, &mut st);
}

fn retry_locked(g: &'static Gpu, st: &mut SubmitState) {
    take_recycled(g, st);
    loop {
        let ready: Vec<u64> = st.parked.iter().filter(|(_, p)| exec::wait_passes(g, &p.stream)).map(|(k, _)| *k).collect();
        if ready.is_empty() {
            return;
        }
        for q in ready {
            let p = st.parked.remove(&q).unwrap();
            let ll = st.qstates.get(&q).map(|s| s.origin_ll).unwrap_or(true);
            run_stream(g, st, q, ll, p.stream, p.receipts);
        }
    }
}

/// A CPU event signal: a parked queue may run on.
pub fn event_signalled() {
    if let Some(g) = Gpu::try_get() {
        let any = !g.q.sub.lock().unwrap().parked.is_empty();
        if any {
            retry_parked(g);
        }
    }
}

/// nvnQueueFenceSync: the point covering everything submitted to `qserial` (0 = every queue) so far.
pub fn queue_fence(qserial: u64) -> u64 {
    let Some(g) = Gpu::try_get() else { return 0 };
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
    if fence & RECEIPT == 0 {
        return;
    }
    let Some(g) = Gpu::try_get() else { return };
    if g.q.receipts.lock().unwrap().contains_key(&fence) {
        return;
    }
    crate::log::red_once("a queue waits on a sync of a parked queue: not ordered yet (receipt waits across queues)");
}

fn physical(g: &Gpu, f: u64) -> Option<u64> {
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
    super::present::note_present();
    let mut st = g.q.sub.lock().unwrap();
    take_recycled(g, &mut st);
    if st.parked.contains_key(&qserial) {
        crate::log::red_once("present on a parked queue: shown when the queue runs on is not implemented; presented now");
    }
    let mut sub = new_sub(g, &mut st);
    let dirty = super::mem::begin_submit(g, &mut sub);
    g.image_begin_submit(&mut sub, &dirty);
    super::present::record_present(g, &mut sub, t, origin_ll, crop);
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
    let mut st = g.q.sub.lock().unwrap();
    take_recycled(g, &mut st);
    let mut sub = new_sub(g, &mut st);
    let dirty = super::mem::begin_submit(g, &mut sub);
    g.image_begin_submit(&mut sub, &dirty);
    {
        let mut imgs = g.images.lock().unwrap();
        f(g, &mut sub, &mut imgs);
    }
    submit_sub(g, sub)
}

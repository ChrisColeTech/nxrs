//! Deferred recording (`NXGFX_DEFER=1`, off by default as in Turbo; Turbo `vk_defer.c`): the replay's Vulkan commands
//! for the submission being recorded are made by a recorder thread of their own, so translating the next draws (state,
//! bindings, descriptors) overlaps the driver's command recording.
//!
//! How: the device and its extension loaders are loaded through a shadow `vkGetDeviceProcAddr` that hands out shims for
//! every `vkCmd*` this layer uses. A shim aimed at the command buffer being deferred (`begin`, the submission's main
//! buffer) on the replay thread either queues the call with copies of what its pointers point at (state, binds, draws,
//! dispatches: the per-draw stream), or, for the rarer ones (barriers, passes, copies, clears, queries), first waits
//! for the recorder to catch up and then calls the driver directly; so the buffer only ever has one thread recording
//! into it and the commands keep their order. Any other command buffer, or another thread, goes straight to the
//! driver. `end` waits for the recorder before the buffer is ended. Objects a queued command names live until the
//! GPU has completed the submission (views, buffers and sets are released after use), so the copies stay valid.

use ash::vk;
use ash::vk::Handle;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

/// The command buffer being deferred (raw handle; 0 none).
static CB: AtomicU64 = AtomicU64::new(0);
/// The replay thread's id, while it defers.
static OWNER: AtomicU64 = AtomicU64::new(0);

fn thread_key() -> u64 {
    thread_local!(static K: u64 = {
        static N: AtomicU64 = AtomicU64::new(1);
        N.fetch_add(1, Ordering::Relaxed)
    });
    K.with(|k| *k)
}

/// Deferred recording is wanted (`NXGFX_DEFER=1`), read once at device creation.
pub fn wanted() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match std::env::var("NXGFX_DEFER") {
        Ok(v) => v == "1",
        Err(_) => nxconfig::tristate("vk_defer").unwrap_or(false),
    })
}

static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The shims are in the device's tables (the device was loaded through `shadow_instance`).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

// ---------- queued commands ----------

type Exec = unsafe fn(usize, vk::CommandBuffer, &[u64; 8]);

enum Cmd {
    /// a command whose arguments are all scalars or handles: the real function, its decoder, the arguments
    Words {
        real: usize,
        exec: Exec,
        cb: vk::CommandBuffer,
        a: [u64; 8],
    },
    /// a command with copied arrays
    Owned(Box<dyn FnOnce() + Send>),
}
unsafe impl Send for Cmd {}

struct Ring {
    q: Mutex<(VecDeque<Vec<Cmd>>, u64)>,
    /// signalled when work arrives (recorder) and when the recorder has run everything (drain)
    work: Condvar,
    idle: Condvar,
}

fn ring() -> &'static Ring {
    static R: OnceLock<Ring> = OnceLock::new();
    R.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("vk recorder".into())
            .spawn(recorder);
        Ring {
            q: Mutex::new((VecDeque::new(), 0)),
            work: Condvar::new(),
            idle: Condvar::new(),
        }
    })
}

/// Commands queued since the last hand-over to the recorder (the replay thread's own batch).
static BATCH: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
const BATCH_N: usize = 128;
pub static QUEUED: AtomicU64 = AtomicU64::new(0);
pub static DRAINS: AtomicU64 = AtomicU64::new(0);

fn recorder() {
    let r = ring();
    loop {
        let batch = {
            let mut g = r.q.lock().unwrap();
            loop {
                if let Some(b) = g.0.pop_front() {
                    break b;
                }
                g = r.work.wait(g).unwrap();
            }
        };
        let n = batch.len() as u64;
        for c in batch {
            match c {
                Cmd::Words { real, exec, cb, a } => unsafe { exec(real, cb, &a) },
                Cmd::Owned(f) => f(),
            }
        }
        let mut g = r.q.lock().unwrap();
        g.1 -= n;
        if g.1 == 0 && g.0.is_empty() {
            r.idle.notify_all();
        }
    }
}

fn hand_over() {
    let batch = std::mem::take(&mut *BATCH.lock().unwrap());
    if batch.is_empty() {
        return;
    }
    let r = ring();
    let mut g = r.q.lock().unwrap();
    g.1 += batch.len() as u64;
    g.0.push_back(batch);
    r.work.notify_one();
}

fn push(c: Cmd) {
    QUEUED.fetch_add(1, Ordering::Relaxed);
    let full = {
        let mut b = BATCH.lock().unwrap();
        b.push(c);
        b.len() >= BATCH_N
    };
    if full {
        hand_over();
    }
}

/// Waits until the recorder has made every queued command.
pub fn drain() {
    hand_over();
    let r = ring();
    let mut g = r.q.lock().unwrap();
    while g.1 != 0 || !g.0.is_empty() {
        g = r.idle.wait(g).unwrap();
    }
}

fn deferring(cb: vk::CommandBuffer) -> bool {
    let c = CB.load(Ordering::Relaxed);
    c != 0 && c == cb.as_raw() && OWNER.load(Ordering::Relaxed) == thread_key()
}

/// The replay thread starts recording `cb` (a submission's main buffer): its commands are deferred from here on.
pub fn begin(cb: vk::CommandBuffer) {
    if !active() {
        return;
    }
    OWNER.store(thread_key(), Ordering::Relaxed);
    CB.store(cb.as_raw(), Ordering::Release);
}

/// Before `cb` is ended: the recorder catches up and deferring stops.
pub fn end(cb: vk::CommandBuffer) {
    if !active() || CB.load(Ordering::Relaxed) != cb.as_raw() {
        return;
    }
    if OWNER.load(Ordering::Relaxed) == thread_key() {
        drain();
    }
    CB.store(0, Ordering::Release);
}

// ---------- argument words ----------

trait W: Copy {
    fn w(self) -> u64;
    fn f(w: u64) -> Self;
}
impl W for u32 {
    fn w(self) -> u64 {
        self as u64
    }
    fn f(w: u64) -> Self {
        w as u32
    }
}
impl W for i32 {
    fn w(self) -> u64 {
        self as u32 as u64
    }
    fn f(w: u64) -> Self {
        w as u32 as i32
    }
}
impl W for u64 {
    fn w(self) -> u64 {
        self
    }
    fn f(w: u64) -> Self {
        w
    }
}
impl W for f32 {
    fn w(self) -> u64 {
        self.to_bits() as u64
    }
    fn f(w: u64) -> Self {
        f32::from_bits(w as u32)
    }
}
macro_rules! w_handle {
    ($($t:ty),*) => {$(impl W for $t { fn w(self) -> u64 { self.as_raw() } fn f(w: u64) -> Self { <$t>::from_raw(w) } })*};
}
w_handle!(vk::Buffer, vk::Pipeline, vk::PipelineLayout);
macro_rules! w_enum {
    ($($t:ty),*) => {$(impl W for $t { fn w(self) -> u64 { self.as_raw() as u32 as u64 } fn f(w: u64) -> Self { <$t>::from_raw(w as u32 as i32) } })*};
}
w_enum!(
    vk::PipelineBindPoint,
    vk::IndexType,
    vk::FrontFace,
    vk::PrimitiveTopology,
    vk::CompareOp,
    vk::StencilOp,
    vk::LogicOp,
    vk::PolygonMode
);
macro_rules! w_flags {
    ($($t:ty),*) => {$(impl W for $t { fn w(self) -> u64 { self.as_raw() as u64 } fn f(w: u64) -> Self { <$t>::from_raw(w as _) } })*};
}
w_flags!(
    vk::CullModeFlags,
    vk::StencilFaceFlags,
    vk::SampleCountFlags
);

// ---------- the shims ----------

/// Scalar commands: queued as words when deferring, else straight to the driver.
macro_rules! scalar {
    ($( $vk:literal => $shim:ident ( $( $a:ident : $t:ty ),* ); )*) => {
        $(
            #[allow(non_snake_case)]
            unsafe extern "system" fn $shim(cb: vk::CommandBuffer $(, $a: $t)*) {
                type F = unsafe extern "system" fn(vk::CommandBuffer $(, $t)*);
                let real = real_of($vk);
                if deferring(cb) {
                    #[allow(unused_variables, unused_mut)]
                    unsafe fn exec(real: usize, cb: vk::CommandBuffer, a: &[u64; 8]) {
                        let mut _i = 0usize;
                        $( let $a: $t = { let v = <$t as W>::f(a[_i]); _i += 1; v }; )*
                        let f: F = core::mem::transmute(real);
                        f(cb $(, $a)*)
                    }
                    #[allow(unused_mut)]
                    let mut a = [0u64; 8];
                    let mut _i = 0usize;
                    $( a[_i] = $a.w(); _i += 1; )*
                    push(Cmd::Words { real, exec, cb, a });
                } else {
                    let f: F = core::mem::transmute(real);
                    f(cb $(, $a)*)
                }
            }
        )*
        fn scalar_table() -> Vec<(&'static str, usize)> { vec![$( ($vk, $shim as *const () as usize) ),*] }
    };
}

/// Commands passed through: the recorder catches up first when the buffer is deferred.
macro_rules! drained {
    ($( $vk:literal => $shim:ident ( $( $a:ident : $t:ty ),* ); )*) => {
        $(
            #[allow(non_snake_case)]
            unsafe extern "system" fn $shim(cb: vk::CommandBuffer $(, $a: $t)*) {
                type F = unsafe extern "system" fn(vk::CommandBuffer $(, $t)*);
                if deferring(cb) {
                    DRAINS.fetch_add(1, Ordering::Relaxed);
                    drain();
                }
                let f: F = core::mem::transmute(real_of($vk));
                f(cb $(, $a)*)
            }
        )*
        fn drained_table() -> Vec<(&'static str, usize)> { vec![$( ($vk, $shim as *const () as usize) ),*] }
    };
}

type B32 = u32;
type P = *const core::ffi::c_void;

scalar! {
    "vkCmdBindPipeline" => s_bind_pipeline(bp: vk::PipelineBindPoint, p: vk::Pipeline);
    "vkCmdDispatch" => s_dispatch(x: u32, y: u32, z: u32);
    "vkCmdDispatchIndirect" => s_dispatch_indirect(b: vk::Buffer, o: u64);
    "vkCmdDraw" => s_draw(v: u32, i: u32, fv: u32, fi: u32);
    "vkCmdDrawIndexed" => s_draw_indexed(n: u32, i: u32, fi: u32, vo: i32, fin: u32);
    "vkCmdDrawIndirect" => s_draw_indirect(b: vk::Buffer, o: u64, n: u32, s: u32);
    "vkCmdDrawIndexedIndirect" => s_draw_indexed_indirect(b: vk::Buffer, o: u64, n: u32, s: u32);
    "vkCmdDrawIndirectCount" => s_draw_indirect_count(b: vk::Buffer, o: u64, cb2: vk::Buffer, co: u64, m: u32, s: u32);
    "vkCmdDrawIndexedIndirectCount" => s_draw_indexed_indirect_count(b: vk::Buffer, o: u64, cb2: vk::Buffer, co: u64, m: u32, s: u32);
    "vkCmdDrawMeshTasksEXT" => s_mesh(x: u32, y: u32, z: u32);
    "vkCmdDrawMeshTasksIndirectEXT" => s_mesh_indirect(b: vk::Buffer, o: u64, n: u32, s: u32);
    "vkCmdDrawMeshTasksIndirectCountEXT" => s_mesh_indirect_count(b: vk::Buffer, o: u64, cb2: vk::Buffer, co: u64, m: u32, s: u32);
    "vkCmdBindIndexBuffer" => s_bind_index(b: vk::Buffer, o: u64, t: vk::IndexType);
    "vkCmdSetLineWidth" => s_line_width(w: f32);
    "vkCmdSetDepthBias" => s_depth_bias(c: f32, cl: f32, sl: f32);
    "vkCmdSetDepthBounds" => s_depth_bounds(a: f32, b: f32);
    "vkCmdSetStencilCompareMask" => s_st_cmask(f: vk::StencilFaceFlags, m: u32);
    "vkCmdSetStencilWriteMask" => s_st_wmask(f: vk::StencilFaceFlags, m: u32);
    "vkCmdSetStencilReference" => s_st_ref(f: vk::StencilFaceFlags, m: u32);
    "vkCmdSetCullMode" => s_cull(m: vk::CullModeFlags);
    "vkCmdSetFrontFace" => s_front(f: vk::FrontFace);
    "vkCmdSetPrimitiveTopology" => s_topology(t: vk::PrimitiveTopology);
    "vkCmdSetDepthTestEnable" => s_dt(e: B32);
    "vkCmdSetDepthWriteEnable" => s_dw(e: B32);
    "vkCmdSetDepthCompareOp" => s_dcmp(o: vk::CompareOp);
    "vkCmdSetDepthBoundsTestEnable" => s_dbt(e: B32);
    "vkCmdSetStencilTestEnable" => s_st(e: B32);
    "vkCmdSetStencilOp" => s_st_op(f: vk::StencilFaceFlags, a: vk::StencilOp, b: vk::StencilOp, c: vk::StencilOp, d: vk::CompareOp);
    "vkCmdSetRasterizerDiscardEnable" => s_rd(e: B32);
    "vkCmdSetDepthBiasEnable" => s_dbe(e: B32);
    "vkCmdSetPrimitiveRestartEnable" => s_restart(e: B32);
    "vkCmdSetLogicOpEXT" => s_logic(o: vk::LogicOp);
    "vkCmdSetPatchControlPointsEXT" => s_patch(n: u32);
    "vkCmdSetPolygonModeEXT" => s_poly(m: vk::PolygonMode);
    "vkCmdSetRasterizationSamplesEXT" => s_samples(s: vk::SampleCountFlags);
    "vkCmdSetAlphaToCoverageEnableEXT" => s_a2c(e: B32);
    "vkCmdSetLogicOpEnableEXT" => s_logic_en(e: B32);
    "vkCmdSetDepthClampEnableEXT" => s_dclamp(e: B32);
    "vkCmdSetDepthClipNegativeOneToOneEXT" => s_dclip(e: B32);
    "vkCmdSetCheckpointNV" => s_checkpoint(m: u64);
    "vkCmdFillBuffer" => s_fill(b: u64, o: u64, n: u64, v: u32);
    "vkCmdWriteTimestamp2" => s_ts2(st: u64, q: u64, i: u32);
    "vkCmdResetQueryPool" => s_reset_q(q: u64, f: u32, n: u32);
    "vkCmdBeginQuery" => s_begin_q(q: u64, i: u32, f: u32);
    "vkCmdEndQuery" => s_end_q(q: u64, i: u32);
    "vkCmdEndRendering" => s_end_r();
}

drained! {
    "vkCmdUpdateBuffer" => d_update(b: u64, o: u64, n: u64, d: P);
    "vkCmdPushDescriptorSetKHR" => d_push_desc(bp: i32, l: u64, s: u32, n: u32, w: P);
    "vkCmdBindDescriptorBuffersEXT" => d_bind_db(n: u32, i: P);
    "vkCmdBindTransformFeedbackBuffersEXT" => d_xfb_bind(f: u32, n: u32, b: P, o: P, s: P);
    "vkCmdBeginTransformFeedbackEXT" => d_xfb_begin(f: u32, n: u32, b: P, o: P);
    "vkCmdEndTransformFeedbackEXT" => d_xfb_end(f: u32, n: u32, b: P, o: P);
    "vkCmdExecuteCommands" => d_exec(n: u32, c: P);
    "vkCmdSetEvent2" => d_set_event2(e: u64, i: P);
    "vkCmdWaitEvents2" => d_wait_events2(n: u32, e: P, i: P);
    "vkCmdResetEvent2" => d_reset_event2(e: u64, s: u64);
    "vkCmdCopyQueryPoolResults" => d_copy_q(q: u64, f: u32, n: u32, b: u64, o: u64, s: u64, fl: u32);
    "vkCmdPipelineBarrier" => d_barrier(s: u32, d: u32, dep: u32, nm: u32, m: P, nb: u32, b: P, ni: u32, i: P);
    "vkCmdBindVertexBuffers2" => d_bind_vb2(f: u32, n: u32, b: P, o: P, sz: P, st: P);
    "vkCmdSetViewport" => d_set_vp(f: u32, n: u32, v: P);
    "vkCmdSetScissor" => d_set_sc(f: u32, n: u32, v: P);
    "vkCmdBuildAccelerationStructuresKHR" => d_build_as(n: u32, i: P, r: P);
}

/// Commands with arrays: queued with copies.
unsafe fn copy<T: Copy>(p: *const T, n: u32) -> Vec<T> {
    if p.is_null() || n == 0 {
        Vec::new()
    } else {
        core::slice::from_raw_parts(p, n as usize).to_vec()
    }
}

struct SendVec<T>(Vec<T>);
unsafe impl<T> Send for SendVec<T> {}

macro_rules! real {
    ($vk:literal, $($t:ty),*) => {{
        type F = unsafe extern "system" fn(vk::CommandBuffer $(, $t)*);
        let f: F = core::mem::transmute(real_of($vk));
        f
    }};
}

unsafe extern "system" fn a_viewports(cb: vk::CommandBuffer, n: u32, v: *const vk::Viewport) {
    let f = real!("vkCmdSetViewportWithCount", u32, *const vk::Viewport);
    if !deferring(cb) {
        return f(cb, n, v);
    }
    let v = copy(v, n);
    push(Cmd::Owned(Box::new(move || f(cb, n, v.as_ptr()))));
}
unsafe extern "system" fn a_scissors(cb: vk::CommandBuffer, n: u32, v: *const vk::Rect2D) {
    let f = real!("vkCmdSetScissorWithCount", u32, *const vk::Rect2D);
    if !deferring(cb) {
        return f(cb, n, v);
    }
    let v = copy(v, n);
    push(Cmd::Owned(Box::new(move || f(cb, n, v.as_ptr()))));
}
unsafe extern "system" fn a_blend_constants(cb: vk::CommandBuffer, c: *const [f32; 4]) {
    let f = real!("vkCmdSetBlendConstants", *const [f32; 4]);
    if !deferring(cb) {
        return f(cb, c);
    }
    let c = *c;
    push(Cmd::Owned(Box::new(move || f(cb, &c))));
}
unsafe extern "system" fn a_sample_mask(
    cb: vk::CommandBuffer,
    s: vk::SampleCountFlags,
    m: *const vk::SampleMask,
) {
    let f = real!(
        "vkCmdSetSampleMaskEXT",
        vk::SampleCountFlags,
        *const vk::SampleMask
    );
    if !deferring(cb) {
        return f(cb, s, m);
    }
    let m = copy(m, s.as_raw().div_ceil(32).max(1));
    push(Cmd::Owned(Box::new(move || f(cb, s, m.as_ptr()))));
}
unsafe extern "system" fn a_blend_enable(
    cb: vk::CommandBuffer,
    first: u32,
    n: u32,
    e: *const vk::Bool32,
) {
    let f = real!("vkCmdSetColorBlendEnableEXT", u32, u32, *const vk::Bool32);
    if !deferring(cb) {
        return f(cb, first, n, e);
    }
    let e = copy(e, n);
    push(Cmd::Owned(Box::new(move || f(cb, first, n, e.as_ptr()))));
}
unsafe extern "system" fn a_blend_eq(
    cb: vk::CommandBuffer,
    first: u32,
    n: u32,
    e: *const vk::ColorBlendEquationEXT,
) {
    let f = real!(
        "vkCmdSetColorBlendEquationEXT",
        u32,
        u32,
        *const vk::ColorBlendEquationEXT
    );
    if !deferring(cb) {
        return f(cb, first, n, e);
    }
    let e = copy(e, n);
    push(Cmd::Owned(Box::new(move || f(cb, first, n, e.as_ptr()))));
}
unsafe extern "system" fn a_write_mask(
    cb: vk::CommandBuffer,
    first: u32,
    n: u32,
    e: *const vk::ColorComponentFlags,
) {
    let f = real!(
        "vkCmdSetColorWriteMaskEXT",
        u32,
        u32,
        *const vk::ColorComponentFlags
    );
    if !deferring(cb) {
        return f(cb, first, n, e);
    }
    let e = copy(e, n);
    push(Cmd::Owned(Box::new(move || f(cb, first, n, e.as_ptr()))));
}
unsafe extern "system" fn a_vertex_buffers(
    cb: vk::CommandBuffer,
    first: u32,
    n: u32,
    b: *const vk::Buffer,
    o: *const vk::DeviceSize,
) {
    let f = real!(
        "vkCmdBindVertexBuffers",
        u32,
        u32,
        *const vk::Buffer,
        *const vk::DeviceSize
    );
    if !deferring(cb) {
        return f(cb, first, n, b, o);
    }
    let (b, o) = (copy(b, n), copy(o, n));
    push(Cmd::Owned(Box::new(move || {
        f(cb, first, n, b.as_ptr(), o.as_ptr())
    })));
}
unsafe extern "system" fn a_push_constants(
    cb: vk::CommandBuffer,
    l: vk::PipelineLayout,
    st: vk::ShaderStageFlags,
    off: u32,
    n: u32,
    v: *const core::ffi::c_void,
) {
    let f = real!(
        "vkCmdPushConstants",
        vk::PipelineLayout,
        vk::ShaderStageFlags,
        u32,
        u32,
        *const core::ffi::c_void
    );
    if !deferring(cb) {
        return f(cb, l, st, off, n, v);
    }
    let v = copy(v as *const u8, n);
    push(Cmd::Owned(Box::new(move || {
        f(cb, l, st, off, n, v.as_ptr() as *const _)
    })));
}
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn a_bind_sets(
    cb: vk::CommandBuffer,
    bp: vk::PipelineBindPoint,
    l: vk::PipelineLayout,
    first: u32,
    n: u32,
    s: *const vk::DescriptorSet,
    nd: u32,
    d: *const u32,
) {
    let f = real!(
        "vkCmdBindDescriptorSets",
        vk::PipelineBindPoint,
        vk::PipelineLayout,
        u32,
        u32,
        *const vk::DescriptorSet,
        u32,
        *const u32
    );
    if !deferring(cb) {
        return f(cb, bp, l, first, n, s, nd, d);
    }
    let (s, d) = (copy(s, n), copy(d, nd));
    push(Cmd::Owned(Box::new(move || {
        f(cb, bp, l, first, n, s.as_ptr(), nd, d.as_ptr())
    })));
}
unsafe extern "system" fn a_db_offsets(
    cb: vk::CommandBuffer,
    bp: vk::PipelineBindPoint,
    l: vk::PipelineLayout,
    first: u32,
    n: u32,
    i: *const u32,
    o: *const vk::DeviceSize,
) {
    let f = real!(
        "vkCmdSetDescriptorBufferOffsetsEXT",
        vk::PipelineBindPoint,
        vk::PipelineLayout,
        u32,
        u32,
        *const u32,
        *const vk::DeviceSize
    );
    if !deferring(cb) {
        return f(cb, bp, l, first, n, i, o);
    }
    let (i, o) = (copy(i, n), copy(o, n));
    push(Cmd::Owned(Box::new(move || {
        f(cb, bp, l, first, n, i.as_ptr(), o.as_ptr())
    })));
}
unsafe extern "system" fn a_vertex_input(
    cb: vk::CommandBuffer,
    nb: u32,
    b: *const vk::VertexInputBindingDescription2EXT<'static>,
    na: u32,
    a: *const vk::VertexInputAttributeDescription2EXT<'static>,
) {
    let f = real!(
        "vkCmdSetVertexInputEXT",
        u32,
        *const vk::VertexInputBindingDescription2EXT<'static>,
        u32,
        *const vk::VertexInputAttributeDescription2EXT<'static>
    );
    if !deferring(cb) {
        return f(cb, nb, b, na, a);
    }
    // p_next is null in what this layer passes
    let (b, a) = (SendVec(copy(b, nb)), SendVec(copy(a, na)));
    push(Cmd::Owned(Box::new(move || {
        let (b, a) = (b, a);
        f(cb, nb, b.0.as_ptr(), na, a.0.as_ptr())
    })));
}

macro_rules! regions {
    ($shim:ident, $vk:literal, $rt:ty, ($($a:ident : $t:ty),*)) => {
        unsafe extern "system" fn $shim(cb: vk::CommandBuffer $(, $a: $t)*, n: u32, r: *const $rt) {
            let f = real!($vk $(, $t)*, u32, *const $rt);
            if !deferring(cb) {
                return f(cb $(, $a)*, n, r);
            }
            let r = SendVec(copy(r, n));
            push(Cmd::Owned(Box::new(move || {
                let r = r;
                f(cb $(, $a)*, n, r.0.as_ptr())
            })));
        }
    };
}
regions!(a_copy_buf, "vkCmdCopyBuffer", vk::BufferCopy, (s: vk::Buffer, d: vk::Buffer));
regions!(a_copy_img, "vkCmdCopyImage", vk::ImageCopy, (s: vk::Image, sl: vk::ImageLayout, d: vk::Image, dl: vk::ImageLayout));
regions!(a_copy_b2i, "vkCmdCopyBufferToImage", vk::BufferImageCopy, (s: vk::Buffer, d: vk::Image, dl: vk::ImageLayout));
regions!(a_copy_i2b, "vkCmdCopyImageToBuffer", vk::BufferImageCopy, (s: vk::Image, sl: vk::ImageLayout, d: vk::Buffer));

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn a_blit(
    cb: vk::CommandBuffer,
    s: vk::Image,
    sl: vk::ImageLayout,
    d: vk::Image,
    dl: vk::ImageLayout,
    n: u32,
    r: *const vk::ImageBlit,
    flt: vk::Filter,
) {
    let f = real!(
        "vkCmdBlitImage",
        vk::Image,
        vk::ImageLayout,
        vk::Image,
        vk::ImageLayout,
        u32,
        *const vk::ImageBlit,
        vk::Filter
    );
    if !deferring(cb) {
        return f(cb, s, sl, d, dl, n, r, flt);
    }
    let r = SendVec(copy(r, n));
    push(Cmd::Owned(Box::new(move || {
        let r = r;
        f(cb, s, sl, d, dl, n, r.0.as_ptr(), flt)
    })));
}
unsafe extern "system" fn a_clear_color(
    cb: vk::CommandBuffer,
    i: vk::Image,
    l: vk::ImageLayout,
    c: *const vk::ClearColorValue,
    n: u32,
    r: *const vk::ImageSubresourceRange,
) {
    let f = real!(
        "vkCmdClearColorImage",
        vk::Image,
        vk::ImageLayout,
        *const vk::ClearColorValue,
        u32,
        *const vk::ImageSubresourceRange
    );
    if !deferring(cb) {
        return f(cb, i, l, c, n, r);
    }
    let (c, r) = (*c, SendVec(copy(r, n)));
    push(Cmd::Owned(Box::new(move || {
        let r = r;
        f(cb, i, l, &c, n, r.0.as_ptr())
    })));
}
unsafe extern "system" fn a_clear_ds(
    cb: vk::CommandBuffer,
    i: vk::Image,
    l: vk::ImageLayout,
    c: *const vk::ClearDepthStencilValue,
    n: u32,
    r: *const vk::ImageSubresourceRange,
) {
    let f = real!(
        "vkCmdClearDepthStencilImage",
        vk::Image,
        vk::ImageLayout,
        *const vk::ClearDepthStencilValue,
        u32,
        *const vk::ImageSubresourceRange
    );
    if !deferring(cb) {
        return f(cb, i, l, c, n, r);
    }
    let (c, r) = (*c, SendVec(copy(r, n)));
    push(Cmd::Owned(Box::new(move || {
        let r = r;
        f(cb, i, l, &c, n, r.0.as_ptr())
    })));
}
unsafe extern "system" fn a_clear_att(
    cb: vk::CommandBuffer,
    n: u32,
    a: *const vk::ClearAttachment,
    nr: u32,
    r: *const vk::ClearRect,
) {
    let f = real!(
        "vkCmdClearAttachments",
        u32,
        *const vk::ClearAttachment,
        u32,
        *const vk::ClearRect
    );
    if !deferring(cb) {
        return f(cb, n, a, nr, r);
    }
    let (a, r) = (SendVec(copy(a, n)), SendVec(copy(r, nr)));
    push(Cmd::Owned(Box::new(move || {
        let (a, r) = (a, r);
        f(cb, n, a.0.as_ptr(), nr, r.0.as_ptr())
    })));
}
/// A barrier: its three arrays copied (this layer chains nothing to them; a chained one is drained instead).
unsafe extern "system" fn a_barrier2(cb: vk::CommandBuffer, d: *const vk::DependencyInfo<'static>) {
    let f = real!("vkCmdPipelineBarrier2", *const vk::DependencyInfo<'static>);
    if !deferring(cb) {
        return f(cb, d);
    }
    if !(*d).p_next.is_null() {
        drain();
        return f(cb, d);
    }
    let di = *d;
    let m = SendVec(copy(di.p_memory_barriers, di.memory_barrier_count));
    let b = SendVec(copy(
        di.p_buffer_memory_barriers,
        di.buffer_memory_barrier_count,
    ));
    let i = SendVec(copy(
        di.p_image_memory_barriers,
        di.image_memory_barrier_count,
    ));
    let flags = di.dependency_flags;
    push(Cmd::Owned(Box::new(move || {
        let (m, b, i) = (m, b, i);
        let mut d = vk::DependencyInfo::default();
        d.dependency_flags = flags;
        (d.memory_barrier_count, d.p_memory_barriers) = (m.0.len() as u32, m.0.as_ptr());
        (d.buffer_memory_barrier_count, d.p_buffer_memory_barriers) =
            (b.0.len() as u32, b.0.as_ptr());
        (d.image_memory_barrier_count, d.p_image_memory_barriers) =
            (i.0.len() as u32, i.0.as_ptr());
        f(cb, &d)
    })));
}
/// A pass: its attachments copied (a chained pass info is drained instead).
unsafe extern "system" fn a_begin_rendering(
    cb: vk::CommandBuffer,
    ri: *const vk::RenderingInfo<'static>,
) {
    let f = real!("vkCmdBeginRendering", *const vk::RenderingInfo<'static>);
    if !deferring(cb) {
        return f(cb, ri);
    }
    if !(*ri).p_next.is_null() {
        drain();
        return f(cb, ri);
    }
    let r = *ri;
    let colors = SendVec(copy(r.p_color_attachments, r.color_attachment_count));
    let depth = SendVec(if r.p_depth_attachment.is_null() {
        vec![]
    } else {
        vec![*r.p_depth_attachment]
    });
    let stencil = SendVec(if r.p_stencil_attachment.is_null() {
        vec![]
    } else {
        vec![*r.p_stencil_attachment]
    });
    let (flags, area, layers, mask) = (r.flags, r.render_area, r.layer_count, r.view_mask);
    push(Cmd::Owned(Box::new(move || {
        let (colors, depth, stencil) = (colors, depth, stencil);
        let mut ri = vk::RenderingInfo::default()
            .flags(flags)
            .render_area(area)
            .layer_count(layers)
            .view_mask(mask);
        (ri.color_attachment_count, ri.p_color_attachments) =
            (colors.0.len() as u32, colors.0.as_ptr());
        ri.p_depth_attachment = depth.0.first().map_or(core::ptr::null(), |d| d as *const _);
        ri.p_stencil_attachment = stencil
            .0
            .first()
            .map_or(core::ptr::null(), |s| s as *const _);
        f(cb, &ri)
    })));
}

fn array_table() -> Vec<(&'static str, usize)> {
    vec![
        ("vkCmdCopyBuffer", a_copy_buf as *const () as usize),
        ("vkCmdCopyImage", a_copy_img as *const () as usize),
        ("vkCmdCopyBufferToImage", a_copy_b2i as *const () as usize),
        ("vkCmdCopyImageToBuffer", a_copy_i2b as *const () as usize),
        ("vkCmdBlitImage", a_blit as *const () as usize),
        ("vkCmdClearColorImage", a_clear_color as *const () as usize),
        (
            "vkCmdClearDepthStencilImage",
            a_clear_ds as *const () as usize,
        ),
        ("vkCmdClearAttachments", a_clear_att as *const () as usize),
        ("vkCmdPipelineBarrier2", a_barrier2 as *const () as usize),
        (
            "vkCmdBeginRendering",
            a_begin_rendering as *const () as usize,
        ),
        (
            "vkCmdSetViewportWithCount",
            a_viewports as *const () as usize,
        ),
        ("vkCmdSetScissorWithCount", a_scissors as *const () as usize),
        (
            "vkCmdSetBlendConstants",
            a_blend_constants as *const () as usize,
        ),
        ("vkCmdSetSampleMaskEXT", a_sample_mask as *const () as usize),
        (
            "vkCmdSetColorBlendEnableEXT",
            a_blend_enable as *const () as usize,
        ),
        (
            "vkCmdSetColorBlendEquationEXT",
            a_blend_eq as *const () as usize,
        ),
        (
            "vkCmdSetColorWriteMaskEXT",
            a_write_mask as *const () as usize,
        ),
        (
            "vkCmdBindVertexBuffers",
            a_vertex_buffers as *const () as usize,
        ),
        ("vkCmdPushConstants", a_push_constants as *const () as usize),
        ("vkCmdBindDescriptorSets", a_bind_sets as *const () as usize),
        (
            "vkCmdSetDescriptorBufferOffsetsEXT",
            a_db_offsets as *const () as usize,
        ),
        (
            "vkCmdSetVertexInputEXT",
            a_vertex_input as *const () as usize,
        ),
    ]
}

// ---------- the shadow vkGetDeviceProcAddr ----------

struct Tables {
    gdpa: vk::PFN_vkGetDeviceProcAddr,
    device: vk::Device,
    /// name -> the real function (filled as the loaders ask)
    real: Mutex<std::collections::HashMap<String, usize>>,
}
static T: OnceLock<Tables> = OnceLock::new();
static MISSES: AtomicUsize = AtomicUsize::new(0);

fn real_of(name: &str) -> usize {
    let t = T.get().expect("deferred recording tables");
    if let Some(&p) = t.real.lock().unwrap().get(name) {
        return p;
    }
    let c = std::ffi::CString::new(name).unwrap();
    let p = unsafe { (t.gdpa)(t.device, c.as_ptr()) }
        .map(|f| f as usize)
        .unwrap_or(0);
    if p == 0 {
        MISSES.fetch_add(1, Ordering::Relaxed);
    }
    t.real.lock().unwrap().insert(name.to_string(), p);
    p
}

unsafe extern "system" fn shadow_gdpa(
    device: vk::Device,
    name: *const core::ffi::c_char,
) -> vk::PFN_vkVoidFunction {
    let t = T.get().unwrap();
    let real = (t.gdpa)(device, name);
    let Ok(n) = std::ffi::CStr::from_ptr(name).to_str() else {
        return real;
    };
    if real.is_none() {
        return None;
    }
    for table in [scalar_table(), drained_table(), array_table()] {
        if let Some(&(_, shim)) = table.iter().find(|(k, _)| *k == n) {
            t.real
                .lock()
                .unwrap()
                .insert(n.to_string(), real.unwrap() as usize);
            return Some(core::mem::transmute::<usize, unsafe extern "system" fn()>(
                shim,
            ));
        }
    }
    if n.starts_with("vkCmd") {
        static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let mut s = SAID.lock().unwrap();
        if !s.iter().any(|x| x == n) {
            s.push(n.to_string());
            if s.len() <= 4 {
                crate::vklog!("deferred recording: {n} is not wrapped (a call on the deferred buffer would not keep its order; this layer does not use it)");
            }
        }
    }
    real
}

/// An instance whose device-level lookups hand out the shims (the real device and loaders are made through it).
pub fn shadow_instance(instance: &ash::Instance, device: vk::Device) -> ash::Instance {
    let _ = T.set(Tables {
        gdpa: instance.fp_v1_0().get_device_proc_addr,
        device,
        real: Mutex::new(Default::default()),
    });
    let mut f0 = instance.fp_v1_0().clone();
    f0.get_device_proc_addr = shadow_gdpa;
    ACTIVE.store(true, Ordering::Relaxed);
    let _ = ring();
    crate::vklog!("deferred recording: the replay's commands are recorded on a thread of their own (NXGFX_DEFER=1)");
    ash::Instance::from_parts_1_3(
        instance.handle(),
        f0,
        instance.fp_v1_1().clone(),
        instance.fp_v1_3().clone(),
    )
}

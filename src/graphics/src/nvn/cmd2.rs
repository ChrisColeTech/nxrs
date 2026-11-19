//! Command-buffer entry points of NVN2 and the rarer NVN ones: push constants, deferred binds, conditional render,
//! the alpha test reference, depth bounds, sample mask, point size, tessellation levels, multi-draws with a GPU count,
//! DrawTexture, mesh-task draws, transform feedback, report values, downsampling, depth resolves, and the hints the
//! host has nothing to do for. Records are replayed by `vk::exec`.

use super::cmd::{bind_handles_deferred, rec, rec_t, st_host};
use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;

/// NVN2 push constants: `size` bytes at `offset` of the graphics or compute group's c[24] (block b at b * 0x100).
/// `deferred`: `src` names the bytes, read when the queue runs the record.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_push_const {
    pub compute: u16,
    pub offset: u16,
    pub size: u32,
    pub deferred: u32,
    pub pad: u32,
    pub src: u64,
}
pub const PUSH_BLOCK: usize = 0x100;
pub const PUSH_GROUP: usize = 0x800;

/// SetRenderEnable(Conditional): mode 2 render, 3 do not, 0 / 1 render if the two 64-bit words at `addr` are equal /
/// differ (NVN's conditional render modes), read when the queue runs the record.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_render_cond {
    pub mode: i32,
    pub pad: i32,
    pub addr: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_f32x4 {
    pub v: [f32; 4],
    pub i: [i32; 4],
}

/// MultiDraw{Arrays,Elements}IndirectCount: up to `max` records of `stride` bytes at `ind`, the count at `cnt`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_multi_draw {
    pub prim: i32,
    pub itype: i32,
    pub ibuf: u64,
    pub ind: u64,
    pub cnt: u64,
    pub max: i32,
    pub stride: i32,
}

/// DrawTexture: a texture handle drawn into the bound targets, destination and source rectangles.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_draw_tex {
    pub handle: u64,
    pub dst: [f32; 4],
    pub src: [f32; 4],
}

/// Transform feedback: `what` 0 begin, 1 end, 2 pause, 3 resume; buffers bound per index.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_tf_state {
    pub what: i32,
    pub pad: i32,
    pub addr: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_tf_bind {
    pub index: i32,
    pub pad: i32,
    pub addr: u64,
    pub size: u64,
}

/// Mesh-task draws: count workgroups from first (`ind`: the {count, first} record; 3D: x y z), `n` records of
/// `stride` from a multi-draw, with the count at `cnt` when given.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_mesh {
    pub first: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub ind: u64,
    pub cnt: u64,
    pub n: i32,
    pub stride: i32,
    pub three_d: i32,
    pub pad: i32,
}

/// ReportValue: the 32-bit value written at `addr` once the queue's work before it is done.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_report_value {
    pub value: u32,
    pub pad: u32,
    pub addr: u64,
}

/// Downsample / ResolveDepthBuffer: `src` into `dst` (snapshots of the texture objects), views optional.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_tex2 {
    pub src: tex_t,
    pub dst: tex_t,
    pub sv: view_t,
    pub dv: view_t,
    pub has_sv: u8,
    pub has_dv: u8,
    pub kind: u8,
}

/// Deferred buffer binds: `n` NVNbufferRange records at `src`, read when the queue runs the record.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_bufs_deferred {
    pub ssbo: i32,
    pub stage: i32,
    pub first: i32,
    pub n: i32,
    pub src: u64,
}

unsafe fn push_const(c: *mut NVNcommandBuffer, compute: bool, w: u64, off: u64, n: u64, data: *const u8, deferred: bool) {
    let at = w as usize * PUSH_BLOCK + off as usize;
    if at + n as usize > PUSH_GROUP || data.is_null() {
        crate::log::red_once(&format!("push constants past their group: block {w} offset {off:#x} size {n:#x}"));
        return;
    }
    let size = core::mem::size_of::<p_push_const>();
    let extra = if deferred { 0 } else { n as usize };
    let p = rec(c, op::PUSH_CONST, size + extra);
    let h = p as *mut p_push_const;
    *h = p_push_const { compute: compute as u16, offset: at as u16, size: n as u32, deferred: deferred as u32, pad: 0, src: data as u64 };
    if !deferred {
        core::ptr::copy_nonoverlapping(data, p.add(size), n as usize);
    }
}

unsafe fn tex_pair(c: *mut NVNcommandBuffer, op: u32, kind: u8, s: *const NVNtexture, sv: *const NVNtextureView, d: *const NVNtexture, dv: *const NVNtextureView) {
    let r = rec_t::<p_tex2>(c, op);
    (*r).src = *(s as *const tex_t);
    (*r).dst = *(d as *const tex_t);
    (*r).has_sv = !sv.is_null() as u8;
    (*r).has_dv = !dv.is_null() as u8;
    if !sv.is_null() {
        (*r).sv = *(sv as *const view_t);
    }
    if !dv.is_null() {
        (*r).dv = *(dv as *const view_t);
    }
    (*r).kind = kind;
}

unsafe fn mesh(c: *mut NVNcommandBuffer, m: p_mesh) {
    *rec_t::<p_mesh>(c, op::MESH) = m;
}

nvn_fns! {
    // ---------- NVN2 push constants (driver bank 24) ----------
    fn nvnCommandBufferPushGraphicsConstants(c: *mut NVNcommandBuffer, w: u64, off: u64, n: u64, data: *const u8) { push_const(c, false, w, off, n, data, false); }
    fn nvnCommandBufferPushComputeConstants(c: *mut NVNcommandBuffer, w: u64, off: u64, n: u64, data: *const u8) { push_const(c, true, w, off, n, data, false); }
    fn nvnCommandBufferPushGraphicsConstantsDeferred(c: *mut NVNcommandBuffer, w: u64, off: u64, n: u64, data: *const u8) { push_const(c, false, w, off, n, data, true); }
    fn nvnCommandBufferPushComputeConstantsDeferred(c: *mut NVNcommandBuffer, w: u64, off: u64, n: u64, data: *const u8) { push_const(c, true, w, off, n, data, true); }

    // ---------- deferred binds: the handles / ranges at src as they are when the queue runs ----------
    fn nvnCommandBufferBindTexturesDeferred(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, src: u64) { bind_handles_deferred(c, op::BIND_TEX, st_host(st), first, n, src); }
    fn nvnCommandBufferBindImagesDeferred(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, src: u64) { bind_handles_deferred(c, op::BIND_IMG, st_host(st), first, n, src); }
    fn nvnCommandBufferBindSeparateTexturesDeferred(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, src: u64) { bind_handles_deferred(c, op::BIND_TEX, st_host(st), SEPTEX_SLOT0 + first, n, src); }
    fn nvnCommandBufferBindSeparateSamplersDeferred(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, src: u64) { bind_handles_deferred(c, op::BIND_SAMP, st_host(st), first, n, src); }
    fn nvnCommandBufferBindUniformBuffersDeferred(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, src: u64) {
        *rec_t::<p_bufs_deferred>(c, op::BUFS_DEFERRED) = p_bufs_deferred { ssbo: 0, stage: st_host(st), first, n, src };
    }
    fn nvnCommandBufferBindStorageBuffersDeferred(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, src: u64) {
        *rec_t::<p_bufs_deferred>(c, op::BUFS_DEFERRED) = p_bufs_deferred { ssbo: 1, stage: st_host(st), first, n, src };
    }

    // ---------- conditional render, alpha reference, fixed-function state ----------
    fn nvnCommandBufferSetRenderEnable(c: *mut NVNcommandBuffer, en: u32) {
        *rec_t::<p_render_cond>(c, op::RENDER_COND) = p_render_cond { mode: if en & 0xff != 0 { 2 } else { 3 }, pad: 0, addr: 0 };
    }
    fn nvnCommandBufferSetRenderEnableConditional(c: *mut NVNcommandBuffer, m: i32, a: u64) {
        *rec_t::<p_render_cond>(c, op::RENDER_COND) = p_render_cond { mode: m, pad: 0, addr: a };
    }
    fn nvnCommandBufferSetAlphaRef(c: *mut NVNcommandBuffer, r: f32) { *rec_t::<p_f32x4>(c, op::ALPHA_REF) = p_f32x4 { v: [r, 0.0, 0.0, 0.0], i: [0; 4] }; }
    fn nvnCommandBufferSetDepthBounds(c: *mut NVNcommandBuffer, en: u32, lo: f32, hi: f32) {
        *rec_t::<p_f32x4>(c, op::DEPTH_BOUNDS) = p_f32x4 { v: [lo, hi, 0.0, 0.0], i: [(en & 0xff != 0) as i32, 0, 0, 0] };
    }
    fn nvnCommandBufferSetSampleMask(c: *mut NVNcommandBuffer, m: i32) { *rec_t::<p_f32x4>(c, op::SAMPLE_MASK) = p_f32x4 { v: [0.0; 4], i: [m, 0, 0, 0] }; }
    fn nvnCommandBufferSetPointSize(c: *mut NVNcommandBuffer, s: f32) { *rec_t::<p_f32x4>(c, op::POINT_SIZE) = p_f32x4 { v: [s, 0.0, 0.0, 0.0], i: [0; 4] }; }
    fn nvnCommandBufferSetInnerTessellationLevels(c: *mut NVNcommandBuffer, l: *const f32) {
        *rec_t::<p_f32x4>(c, op::TESS_LEVELS) = p_f32x4 { v: [*l, *l.add(1), 0.0, 0.0], i: [0, 0, 0, 0] };
    }
    fn nvnCommandBufferSetOuterTessellationLevels(c: *mut NVNcommandBuffer, l: *const f32) {
        *rec_t::<p_f32x4>(c, op::TESS_LEVELS) = p_f32x4 { v: [*l, *l.add(1), *l.add(2), *l.add(3)], i: [1, 0, 0, 0] };
    }
    fn nvnCommandBufferSetProvokingVertexMode(c: *mut NVNcommandBuffer, m: i32) { *rec_t::<p_f32x4>(c, op::PROVOKING) = p_f32x4 { v: [0.0; 4], i: [m, 0, 0, 0] }; }
    fn nvnCommandBufferSetViewportSwizzles(c: *mut NVNcommandBuffer, first: i32, n: i32, sw: *const i32) {
        // the swizzle of viewport i (NVN's x, y, z, w selects): stored for the replay, which applies the identity and
        // the two negations a projection flip uses through the viewport itself
        for i in 0..n.clamp(0, 16) {
            let s = core::slice::from_raw_parts(sw.add(4 * i as usize), 4);
            *rec_t::<p_f32x4>(c, op::VP_SWIZZLE) = p_f32x4 { v: [0.0; 4], i: [first + i, s[0] | s[1] << 8 | s[2] << 16 | s[3] << 24, 0, 0] };
        }
    }

    // ---------- draws ----------
    fn nvnCommandBufferMultiDrawArraysIndirectCount(c: *mut NVNcommandBuffer, pr: i32, ind: u64, cnt: u64, max: i32, stride: isize) {
        *rec_t::<p_multi_draw>(c, op::MULTI_DRAW) = p_multi_draw { prim: pr, itype: -1, ibuf: 0, ind, cnt, max, stride: stride as i32 };
    }
    fn nvnCommandBufferMultiDrawElementsIndirectCount(c: *mut NVNcommandBuffer, pr: i32, it: i32, ib: u64, ind: u64, cnt: u64, max: i32, stride: isize) {
        *rec_t::<p_multi_draw>(c, op::MULTI_DRAW) = p_multi_draw { prim: pr, itype: it, ibuf: ib, ind, cnt, max, stride: stride as i32 };
    }
    fn nvnCommandBufferDrawTexture(c: *mut NVNcommandBuffer, h: u64, d: *const f32, s: *const f32) {
        let r = rec_t::<p_draw_tex>(c, op::DRAW_TEX);
        (*r).handle = h;
        (*r).dst.copy_from_slice(core::slice::from_raw_parts(d, 4));
        (*r).src.copy_from_slice(core::slice::from_raw_parts(s, 4));
    }
    fn nvnCommandBufferDrawMeshTasks(c: *mut NVNcommandBuffer, first: i32, count: i32) { mesh(c, p_mesh { first, x: count, y: 1, z: 1, ..Default::default() }); }
    fn nvnCommandBufferDrawMeshTasks3D(c: *mut NVNcommandBuffer, x: i32, y: i32, z: i32) { mesh(c, p_mesh { x, y, z, three_d: 1, ..Default::default() }); }
    fn nvnCommandBufferDrawMeshTasksIndirect(c: *mut NVNcommandBuffer, ind: u64) { mesh(c, p_mesh { ind, n: 1, stride: 8, ..Default::default() }); }
    fn nvnCommandBufferDrawMeshTasks3DIndirect(c: *mut NVNcommandBuffer, ind: u64) { mesh(c, p_mesh { ind, n: 1, stride: 12, three_d: 1, ..Default::default() }); }
    fn nvnCommandBufferMultiDrawMeshTasksIndirect(c: *mut NVNcommandBuffer, ind: u64, n: i32, stride: isize) { mesh(c, p_mesh { ind, n, stride: stride as i32, ..Default::default() }); }
    fn nvnCommandBufferMultiDrawMeshTasks3DIndirect(c: *mut NVNcommandBuffer, ind: u64, n: i32, stride: isize) { mesh(c, p_mesh { ind, n, stride: stride as i32, three_d: 1, ..Default::default() }); }
    fn nvnCommandBufferMultiDrawMeshTasksIndirectCount(c: *mut NVNcommandBuffer, ind: u64, cnt: u64, max: i32, stride: isize) { mesh(c, p_mesh { ind, cnt, n: max, stride: stride as i32, ..Default::default() }); }
    fn nvnCommandBufferMultiDrawMeshTasks3DIndirectCount(c: *mut NVNcommandBuffer, ind: u64, cnt: u64, max: i32, stride: isize) { mesh(c, p_mesh { ind, cnt, n: max, stride: stride as i32, three_d: 1, ..Default::default() }); }

    // ---------- transform feedback ----------
    fn nvnCommandBufferBindTransformFeedbackBuffer(c: *mut NVNcommandBuffer, i: i32, a: u64, n: usize) {
        *rec_t::<p_tf_bind>(c, op::TF_BIND) = p_tf_bind { index: i, pad: 0, addr: a, size: n as u64 };
    }
    fn nvnCommandBufferBindTransformFeedbackBuffers(c: *mut NVNcommandBuffer, first: i32, n: i32, r: *const NVNbufferRange) {
        for i in 0..n.clamp(0, 4) {
            let b = *r.add(i as usize);
            *rec_t::<p_tf_bind>(c, op::TF_BIND) = p_tf_bind { index: first + i, pad: 0, addr: b.address, size: b.size };
        }
    }
    fn nvnCommandBufferBeginTransformFeedback(c: *mut NVNcommandBuffer, a: u64) { *rec_t::<p_tf_state>(c, op::TF_STATE) = p_tf_state { what: 0, pad: 0, addr: a }; }
    fn nvnCommandBufferEndTransformFeedback(c: *mut NVNcommandBuffer, a: u64) { *rec_t::<p_tf_state>(c, op::TF_STATE) = p_tf_state { what: 1, pad: 0, addr: a }; }
    fn nvnCommandBufferPauseTransformFeedback(c: *mut NVNcommandBuffer, a: u64) { *rec_t::<p_tf_state>(c, op::TF_STATE) = p_tf_state { what: 2, pad: 0, addr: a }; }
    fn nvnCommandBufferResumeTransformFeedback(c: *mut NVNcommandBuffer, a: u64) { *rec_t::<p_tf_state>(c, op::TF_STATE) = p_tf_state { what: 3, pad: 0, addr: a }; }
    fn nvnCommandBufferDrawTransformFeedback(c: *mut NVNcommandBuffer, pr: i32, a: u64) {
        // the vertex count the last capture into `a`'s control record wrote, read by the GPU
        *rec_t::<p_tf_state>(c, op::TF_STATE) = p_tf_state { what: 4 | pr << 8, pad: 0, addr: a };
    }

    // ---------- values, copies of textures ----------
    fn nvnCommandBufferReportValue(c: *mut NVNcommandBuffer, v: u32, a: u64) { *rec_t::<p_report_value>(c, op::REPORT_VALUE) = p_report_value { value: v, pad: 0, addr: a }; }
    fn nvnCommandBufferDownsample(c: *mut NVNcommandBuffer, s: *const NVNtexture, d: *const NVNtexture) { tex_pair(c, op::DOWNSAMPLE, 0, s, core::ptr::null(), d, core::ptr::null()); }
    fn nvnCommandBufferDownsampleTextureView(c: *mut NVNcommandBuffer, s: *const NVNtexture, sv: *const NVNtextureView, d: *const NVNtexture, dv: *const NVNtextureView) { tex_pair(c, op::DOWNSAMPLE, 0, s, sv, d, dv); }
    fn nvnCommandBufferTiledDownsample(c: *mut NVNcommandBuffer, s: *const NVNtexture, d: *const NVNtexture) { tex_pair(c, op::DOWNSAMPLE, 0, s, core::ptr::null(), d, core::ptr::null()); }
    fn nvnCommandBufferTiledDownsampleTextureView(c: *mut NVNcommandBuffer, s: *const NVNtexture, sv: *const NVNtextureView, d: *const NVNtexture, dv: *const NVNtextureView) { tex_pair(c, op::DOWNSAMPLE, 0, s, sv, d, dv); }
    fn nvnCommandBufferResolveDepthBuffer(c: *mut NVNcommandBuffer) {
        // NVN resolves the bound multisample depth target into its single-sample alias (the driver's depth resolve)
        rec(c, op::RESOLVE_DEPTH, 8);
    }
}

// Hints the host's Vulkan has no counterpart for (scheduling, occupancy, debug grouping, ZCull compression, tile
// sizes, recording modes): their only effect on the console is speed or tooling, so nothing is recorded.
pub mod hints {
    use super::super::entry::nvn_fns;
    use super::super::types::*;
nvn_fns! {
    fn nvnCommandBufferInsertDebugMarkerCompute(c: *mut NVNcommandBuffer, s: *const u8) { let _ = (c, s); }
    fn nvnCommandBufferInsertDebugMarkerGraphics(c: *mut NVNcommandBuffer, s: *const u8) { let _ = (c, s); }
    fn nvnCommandBufferPushDebugGroupCompute(c: *mut NVNcommandBuffer, s: *const u8) { let _ = (c, s); }
    fn nvnCommandBufferPushDebugGroupGraphics(c: *mut NVNcommandBuffer, s: *const u8) { let _ = (c, s); }
    fn nvnCommandBufferPopDebugGroupCompute(c: *mut NVNcommandBuffer) { let _ = c; }
    fn nvnCommandBufferPopDebugGroupGraphics(c: *mut NVNcommandBuffer) { let _ = c; }
    fn nvnCommandBufferInsertCheckpointStatic(c: *mut NVNcommandBuffer, id: u32, s: *const u8) { let _ = (c, id, s); }
    fn nvnCommandBufferInsertCheckpointDynamic(c: *mut NVNcommandBuffer, id: u32, s: *const u8) { let _ = (c, id, s); }
    fn nvnCommandBufferSetComputeOverlapMode(c: *mut NVNcommandBuffer, m: i32) { let _ = (c, m); }
    fn nvnCommandBufferSetComputeWorkloadPriority(c: *mut NVNcommandBuffer, p: i32) { let _ = (c, p); }
    fn nvnCommandBufferSetGraphicsWorkloadPriority(c: *mut NVNcommandBuffer, p: i32) { let _ = (c, p); }
    fn nvnCommandBufferSetResourceOccupancy(c: *mut NVNcommandBuffer, a: i32, b: i32) { let _ = (c, a, b); }
    fn nvnCommandBufferOverrideSubtileSize(c: *mut NVNcommandBuffer, w: i32, h: i32) { let _ = (c, w, h); }
    fn nvnCommandBufferSetZCullZF32CompressionEnable(c: *mut NVNcommandBuffer, e: u32) { let _ = (c, e); }
    fn nvnCommandBufferSetRecordingMode(c: *mut NVNcommandBuffer, m: i32) { let _ = (c, m); }
    fn nvnCommandBufferSetRepresentativeFragmentTest(c: *mut NVNcommandBuffer, e: u32) { let _ = (c, e); }
    fn nvnCommandBufferSetSubpixelPrecisionBias(c: *mut NVNcommandBuffer, x: i32, y: i32) { let _ = (c, x, y); }
    fn nvnCommandBufferSetColorReductionEnable(c: *mut NVNcommandBuffer, e: u32) { let _ = (c, e); }
    fn nvnCommandBufferSetColorReductionThresholds(c: *mut NVNcommandBuffer, f: i32, t: *const u8) { let _ = (c, f, t); }
    fn nvnCommandBufferSetFloatBlendClamp(c: *mut NVNcommandBuffer, e: u32) { let _ = (c, e); }
    fn nvnCommandBufferSetGeometryProcessingMode(c: *mut NVNcommandBuffer, m: i32) { let _ = (c, m); }
    fn nvnCommandBufferBindShadingRateImage(c: *mut NVNcommandBuffer, v: *const u8, t: *const u8) { let _ = (c, v, t); }
    fn nvnCommandBufferShadingRateImagePalette(c: *mut NVNcommandBuffer, first: i32, n: i32, e: *const u8) { let _ = (c, first, n, e); }
    fn nvnCommandBufferSetShadingRatePaletteMode(c: *mut NVNcommandBuffer, m: i32) { let _ = (c, m); }
    fn nvnCommandBufferBindCoverageModulationTable(c: *mut NVNcommandBuffer, n: i32, e: *const f32) { let _ = (c, n, e); }
    fn nvnCommandBufferSetCommandMemoryCallbackEnabled(c: *mut NVNcommandBuffer, e: u32) { let _ = (c, e); }
    fn nvnCommandBufferSetControlMemoryCallbackEnabled(c: *mut NVNcommandBuffer, e: u32) { let _ = (c, e); }
}
}

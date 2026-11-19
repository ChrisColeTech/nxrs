//! NVN2's DLSS objects: the network builder, the network and the evaluation parameters, packed in place (`obj`).
//! Evaluation (nvnCommandBufferEvaluateDlss) reads `dlss_net_t` and `dlss_params_t`; the network's texture IDs and
//! storage are the driver's own needs, kept here for their getters.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;

unsafe fn nb<'a>(b: *const NVNdlssNetworkBuilder) -> &'a mut dlss_builder_t {
    &mut *(b as *mut dlss_builder_t)
}
/// nvnCommandBufferEvaluateDlss's record: the input colour and output textures (snapshots), the input rectangle and
/// the output size.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_dlss {
    pub color: tex_t,
    pub out: tex_t,
    pub has: u32,
    pub iw: i32,
    pub ih: i32,
    pub ow: i32,
    pub oh: i32,
    pub cbase: [i32; 2],
    pub obase: [i32; 2],
}

unsafe fn pr<'a>(p: *const NVNdlssParameters) -> &'a mut dlss_params_t {
    &mut *(p as *mut dlss_params_t)
}

/// The output is this many times the input, by the builder's method (0: 2x, 1: 3x, 2: 1x).
pub fn dlss_factor(method: i32) -> i32 {
    match method {
        1 => 3,
        2 => 1,
        _ => 2,
    }
}

/// GetOptimalSettings' six values: the optimal input size (the method's), then the smallest input the network takes
/// (a third of the output, DLSS's ultra performance) and the largest (the output itself, DLAA).
pub fn optimal(b: &dlss_builder_t) -> [i32; 6] {
    let f = dlss_factor(b.method);
    let cd = |v: i32, d: i32| (v + d - 1) / d;
    [cd(b.w, f), cd(b.h, f), cd(b.w, 3), cd(b.h, 3), b.w, b.h]
}

unsafe fn set_tex(p: *mut NVNdlssParameters, k: usize, t: *const NVNtexture, v: *const NVNtextureView) {
    let o = pr(p);
    o.tex[k] = t as *const tex_t;
    if v.is_null() {
        o.has_view &= !(1 << k);
    } else {
        o.has_view |= 1 << k;
        crate::log::red_once(&format!("DLSS parameter {k} given a texture view: the whole texture is used"));
    }
}

nvn_fns! {
    // ---------- network builder ----------
    fn nvnDlssNetworkBuilderSetDefaults(b: *mut NVNdlssNetworkBuilder) {
        let dev = nb(b).dev;
        core::ptr::write_bytes(b as *mut u8, 0, core::mem::size_of::<dlss_builder_t>());
        nb(b).dev = dev;
        nb(b).max_eval = 1;
    }
    fn nvnDlssNetworkBuilderSetDevice(b: *mut NVNdlssNetworkBuilder, d: *mut NVNdevice) { nb(b).dev = d; }
    fn nvnDlssNetworkBuilderGetDevice(b: *const NVNdlssNetworkBuilder) -> *mut NVNdevice { nb(b).dev }
    fn nvnDlssNetworkBuilderSetFlags(b: *mut NVNdlssNetworkBuilder, f: u32) { nb(b).flags = f; }
    fn nvnDlssNetworkBuilderGetFlags(b: *const NVNdlssNetworkBuilder) -> u32 { nb(b).flags }
    fn nvnDlssNetworkBuilderSetOutputResolution(b: *mut NVNdlssNetworkBuilder, w: i32, h: i32) { nb(b).w = w; nb(b).h = h; }
    fn nvnDlssNetworkBuilderGetOutputResolution(b: *const NVNdlssNetworkBuilder, w: *mut i32, h: *mut i32) {
        if !w.is_null() { *w = nb(b).w; }
        if !h.is_null() { *h = nb(b).h; }
    }
    fn nvnDlssNetworkBuilderSetMethod(b: *mut NVNdlssNetworkBuilder, m: i32) { nb(b).method = m; }
    fn nvnDlssNetworkBuilderGetMethod(b: *const NVNdlssNetworkBuilder) -> i32 { nb(b).method }
    fn nvnDlssNetworkBuilderSetPreset(b: *mut NVNdlssNetworkBuilder, p: i32) { nb(b).preset = p; }
    fn nvnDlssNetworkBuilderGetPreset(b: *const NVNdlssNetworkBuilder) -> i32 { nb(b).preset }
    fn nvnDlssNetworkBuilderSetMaxEvaluationEntries(b: *mut NVNdlssNetworkBuilder, n: i32) { nb(b).max_eval = n; }
    fn nvnDlssNetworkBuilderGetMaxEvaluationEntries(b: *const NVNdlssNetworkBuilder) -> i32 { nb(b).max_eval }
    /// The texture pool and the 32 IDs the driver's network keeps its own textures at.
    fn nvnDlssNetworkBuilderSetTextureIDs(b: *mut NVNdlssNetworkBuilder, pool: *const NVNtexturePool, ids: *const i32) {
        let o = nb(b);
        o.tex_pool = pool;
        if !ids.is_null() { o.tex_ids = core::ptr::read_unaligned(ids as *const [i32; 32]); }
    }
    fn nvnDlssNetworkBuilderGetTexturePool(b: *const NVNdlssNetworkBuilder) -> *const NVNtexturePool { nb(b).tex_pool }
    fn nvnDlssNetworkBuilderSetStorage(b: *mut NVNdlssNetworkBuilder, m: *mut u8, n: usize) { nb(b).mem = m; nb(b).mem_size = n as u64; }
    fn nvnDlssNetworkBuilderGetMemory(b: *const NVNdlssNetworkBuilder) -> *mut u8 { nb(b).mem }
    fn nvnDlssNetworkBuilderGetMemorySize(b: *const NVNdlssNetworkBuilder) -> usize { nb(b).mem_size as usize }
    /// The scaler keeps its memory on the host: a page, for a game that allocates what it is told.
    fn nvnDlssNetworkBuilderGetStorageSize(b: *const NVNdlssNetworkBuilder) -> usize { let _ = b; 0x1000 }
    fn nvnDlssNetworkBuilderGetOptimalSettings(b: *const NVNdlssNetworkBuilder, o0: *mut i32, o1: *mut i32, o2: *mut i32, o3: *mut i32, o4: *mut i32, o5: *mut i32) -> NVNboolean {
        let v = optimal(nb(b));
        for (p, x) in [o0, o1, o2, o3, o4, o5].into_iter().zip(v) { if !p.is_null() { *p = x; } }
        1
    }

    // ---------- evaluation ----------
    /// nvnCommandBufferEvaluateDlss(cmd, network, parameters): recorded; the replay upscales the input rectangle into
    /// the output (no DLSS network runs on a PC GPU here: a filtered upscale stands in).
    fn nvnCommandBufferEvaluateDlss(c: *mut NVNcommandBuffer, n: *const NVNdlssNetwork, p: *const NVNdlssParameters) {
        let net = &*(n as *const dlss_net_t);
        let pa = pr(p);
        let r = &mut *super::cmd::rec_t::<p_dlss>(c, op::DLSS);
        let (ct, ot) = (pa.tex[DLSS_COLOR], pa.tex[DLSS_OUTPUT]);
        if !ct.is_null() { r.color = *ct; r.has |= 1; }
        if !ot.is_null() { r.out = *ot; r.has |= 2; }
        r.iw = pa.iw;
        r.ih = pa.ih;
        r.ow = net.b.w;
        r.oh = net.b.h;
        r.cbase = pa.base[DLSS_COLOR];
        r.obase = pa.base[DLSS_OUTPUT];
    }

    // ---------- network ----------
    fn nvnDlssNetworkInitialize(n: *mut NVNdlssNetwork, b: *const NVNdlssNetworkBuilder) -> NVNboolean {
        let s = *nb(b);
        *(n as *mut dlss_net_t) = dlss_net_t { magic: MAGIC_DLSS, b: s, serial: super::new_serial() };
        crate::nvnlog!("DLSS network {}x{}, method {} (input 1/{}), preset {}, flags {:#x}", s.w, s.h, s.method, dlss_factor(s.method), s.preset, s.flags);
        1
    }
    fn nvnDlssNetworkFinalize(n: *mut NVNdlssNetwork) { (*(n as *mut dlss_net_t)).magic = 0; }
    /// A new output size for the network (each evaluation still takes its sizes from its parameters and textures).
    fn nvnDlssNetworkResize(n: *mut NVNdlssNetwork, w: i32, h: i32) -> NVNboolean {
        let o = &mut *(n as *mut dlss_net_t);
        o.b.w = w;
        o.b.h = h;
        1
    }

    // ---------- evaluation parameters ----------
    fn nvnDlssParametersSetDefaults(p: *mut NVNdlssParameters) {
        core::ptr::write_bytes(p as *mut u8, 0, core::mem::size_of::<dlss_params_t>());
        let o = pr(p);
        o.pre_exposure = 1.0;
        o.mv = [1.0, 1.0];
    }
    fn nvnDlssParametersSetInputColor(p: *mut NVNdlssParameters, t: *const NVNtexture, v: *const NVNtextureView) { set_tex(p, DLSS_COLOR, t, v); }
    fn nvnDlssParametersSetDepth(p: *mut NVNdlssParameters, t: *const NVNtexture, v: *const NVNtextureView) { set_tex(p, DLSS_DEPTH, t, v); }
    fn nvnDlssParametersSetMotionVectors(p: *mut NVNdlssParameters, t: *const NVNtexture, v: *const NVNtextureView) { set_tex(p, DLSS_MOTION, t, v); }
    fn nvnDlssParametersSetOutputColor(p: *mut NVNdlssParameters, t: *const NVNtexture, v: *const NVNtextureView) { set_tex(p, DLSS_OUTPUT, t, v); }
    fn nvnDlssParametersSetBiasCurrentColorMask(p: *mut NVNdlssParameters, t: *const NVNtexture, v: *const NVNtextureView) { set_tex(p, DLSS_BIAS, t, v); }
    fn nvnDlssParametersSetExposureTexture(p: *mut NVNdlssParameters, t: *const NVNtexture, v: *const NVNtextureView) { set_tex(p, DLSS_EXPOSURE, t, v); }
    fn nvnDlssParametersSetInputSubrectangleSize(p: *mut NVNdlssParameters, w: i32, h: i32) { pr(p).iw = w; pr(p).ih = h; }
    fn nvnDlssParametersGetInputSubrectangleSize(p: *const NVNdlssParameters, w: *mut i32, h: *mut i32) {
        if !w.is_null() { *w = pr(p).iw; }
        if !h.is_null() { *h = pr(p).ih; }
    }
    fn nvnDlssParametersSetInputColorSubrectangleBase(p: *mut NVNdlssParameters, x: i32, y: i32) { pr(p).base[DLSS_COLOR] = [x, y]; }
    fn nvnDlssParametersSetDepthSubrectangleBase(p: *mut NVNdlssParameters, x: i32, y: i32) { pr(p).base[DLSS_DEPTH] = [x, y]; }
    fn nvnDlssParametersSetMotionVectorSubrectangleBase(p: *mut NVNdlssParameters, x: i32, y: i32) { pr(p).base[DLSS_MOTION] = [x, y]; }
    fn nvnDlssParametersSetOutputSubrectangleBase(p: *mut NVNdlssParameters, x: i32, y: i32) { pr(p).base[DLSS_OUTPUT] = [x, y]; }
    fn nvnDlssParametersSetBiasCurrentColorSubrectangleBase(p: *mut NVNdlssParameters, x: i32, y: i32) { pr(p).base[DLSS_BIAS] = [x, y]; }
    fn nvnDlssParametersGetInputColorSubrectangleBase(p: *const NVNdlssParameters, x: *mut i32, y: *mut i32) { get2(pr(p).base[DLSS_COLOR], x, y); }
    fn nvnDlssParametersGetDepthSubrectangleBase(p: *const NVNdlssParameters, x: *mut i32, y: *mut i32) { get2(pr(p).base[DLSS_DEPTH], x, y); }
    fn nvnDlssParametersGetMotionVectorSubrectangleBase(p: *const NVNdlssParameters, x: *mut i32, y: *mut i32) { get2(pr(p).base[DLSS_MOTION], x, y); }
    fn nvnDlssParametersGetOutputSubrectangleBase(p: *const NVNdlssParameters, x: *mut i32, y: *mut i32) { get2(pr(p).base[DLSS_OUTPUT], x, y); }
    fn nvnDlssParametersGetBiasCurrentColorSubrectangleBase(p: *const NVNdlssParameters, x: *mut i32, y: *mut i32) { get2(pr(p).base[DLSS_BIAS], x, y); }
    fn nvnDlssParametersSetMotionVectorScale(p: *mut NVNdlssParameters, x: f32, y: f32) { pr(p).mv = [x, y]; }
    fn nvnDlssParametersGetMotionVectorScale(p: *const NVNdlssParameters, x: *mut f32, y: *mut f32) { get2f(pr(p).mv, x, y); }
    fn nvnDlssParametersSetJitterOffset(p: *mut NVNdlssParameters, x: f32, y: f32) { pr(p).jitter = [x, y]; }
    fn nvnDlssParametersGetJitterOffset(p: *const NVNdlssParameters, x: *mut f32, y: *mut f32) { get2f(pr(p).jitter, x, y); }
    fn nvnDlssParametersSetPreExposure(p: *mut NVNdlssParameters, e: f32) { pr(p).pre_exposure = e; }
    fn nvnDlssParametersGetPreExposure(p: *const NVNdlssParameters) -> f32 { pr(p).pre_exposure }
    fn nvnDlssParametersSetHistoryReset(p: *mut NVNdlssParameters, on: u32) { pr(p).reset = super::b(on) as u8; }
    fn nvnDlssParametersGetHistoryReset(p: *const NVNdlssParameters) -> NVNboolean { pr(p).reset }
    /// Which of the network's frames in flight: evaluations replay in order.
    fn nvnDlssParametersSetEvaluationIndex(p: *mut NVNdlssParameters, i: i32) { pr(p).eval_index = i; }
    fn nvnDlssParametersGetEvaluationIndex(p: *const NVNdlssParameters) -> i32 { pr(p).eval_index }
    // NGX's on-screen debug overlays: kept, not drawn (the host's scaler has none).
    fn nvnDlssParametersSetOverlayDebugLayer(p: *mut NVNdlssParameters, v: i32) { pr(p).overlay[0] = v as u8; }
    fn nvnDlssParametersSetOverlayFullScreen(p: *mut NVNdlssParameters, v: i32) { pr(p).overlay[1] = v as u8; }
    fn nvnDlssParametersSetOverlayShowNans(p: *mut NVNdlssParameters, v: i32) { pr(p).overlay[2] = v as u8; }
    fn nvnDlssParametersSetOverlayJitterDebug(p: *mut NVNdlssParameters, v: i32) { pr(p).overlay[3] = v as u8; }
}

unsafe fn get2(v: [i32; 2], x: *mut i32, y: *mut i32) {
    if !x.is_null() {
        *x = v[0];
    }
    if !y.is_null() {
        *y = v[1];
    }
}
unsafe fn get2f(v: [f32; 2], x: *mut f32, y: *mut f32) {
    if !x.is_null() {
        *x = v[0];
    }
    if !y.is_null() {
        *y = v[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_network_and_parameters() {
        let mut b = NVNdlssNetworkBuilder::boxed();
        let bp = &mut *b as *mut NVNdlssNetworkBuilder;
        unsafe {
            nvnDlssNetworkBuilderSetDefaults(bp);
            assert_eq!(nvnDlssNetworkBuilderGetMaxEvaluationEntries(bp), 1);
            nvnDlssNetworkBuilderSetOutputResolution(bp, 1536, 864);
            let mut o = [0i32; 6];
            let p: Vec<*mut i32> = o.iter_mut().map(|x| x as *mut i32).collect();
            nvnDlssNetworkBuilderGetOptimalSettings(bp, p[0], p[1], p[2], p[3], p[4], p[5]);
            assert_eq!(o, [768, 432, 512, 288, 1536, 864]);
            // Xenoblade Chronicles 2 renders 1024x576 into its 1536x864 network: inside the bounds
            assert!(o[2] <= 1024 && 1024 <= o[4] && o[3] <= 576 && 576 <= o[5]);
            let ids: Vec<i32> = (100..132).collect();
            nvnDlssNetworkBuilderSetTextureIDs(bp, 0x40 as *const NVNtexturePool, ids.as_ptr());
            assert_eq!(nb(bp).tex_ids[31], 131);
            let mut n = NVNdlssNetwork::boxed();
            assert_eq!(nvnDlssNetworkInitialize(&mut *n, bp), 1);
            let s0 = (*(n.0.as_ptr() as *const dlss_net_t)).serial;
            nvnDlssNetworkInitialize(&mut *n, bp);
            assert_ne!((*(n.0.as_ptr() as *const dlss_net_t)).serial, s0);
        }
        let mut q = NVNdlssParameters::boxed();
        let pp = &mut *q as *mut NVNdlssParameters;
        unsafe {
            nvnDlssParametersSetDefaults(pp);
            assert_eq!(nvnDlssParametersGetPreExposure(pp), 1.0);
            nvnDlssParametersSetJitterOffset(pp, -0.25, 0.5);
            nvnDlssParametersSetDepthSubrectangleBase(pp, 3, 4);
            nvnDlssParametersSetHistoryReset(pp, 0x100);
            let (mut x, mut y) = (0f32, 0f32);
            nvnDlssParametersGetJitterOffset(pp, &mut x, &mut y);
            assert_eq!((x, y), (-0.25, 0.5));
            let (mut a, mut c) = (0, 0);
            nvnDlssParametersGetDepthSubrectangleBase(pp, &mut a, &mut c);
            assert_eq!((a, c), (3, 4));
            assert_eq!(nvnDlssParametersGetHistoryReset(pp), 0);
        }
    }
}

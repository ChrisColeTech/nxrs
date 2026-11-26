//! Getters and setters of NVN objects that record nothing: state objects (advanced blend, multisample, depth-stencil),
//! samplers, textures and views, queue builders and queues, windows, event builders, device queries, GL sync interop,
//! debug IDs and the NVN2 acceleration-structure size query. Values live in the objects' own memory (`obj`).

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use std::sync::atomic::Ordering;

// blend_st fields past `blend_field::ADV`: not premultiplied source, overlap mode, destination not normalized
const BF_ADV_NPM: usize = 8;
const BF_ADV_OVL: usize = 9;
const BF_ADV_DST_FREE: usize = 10;

unsafe fn ms<'a>(s: *const NVNmultisampleState) -> &'a mut ms_st {
    &mut *(s as *mut ms_st)
}
fn set_bit(v: &mut u8, bit: u8, on: bool) {
    if on {
        *v |= bit
    } else {
        *v &= !bit
    }
}

/// A sample location coordinate as the hardware keeps it: clamped to [0, 1], 4-bit fixed point (1/16 pixel).
pub fn sl_pack(v: f32) -> u8 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    ((v * 16.0) as u32).min(15) as u8
}
pub fn sl_unpack(n: u8) -> f32 {
    (n & 15) as f32 / 16.0
}

/// The pixel grid the 16-entry sample location table covers: grid width x height
/// = 16 / max(samples, raster samples), width >= height.
pub fn sl_grid(samples: u32) -> (i32, i32) {
    match samples.max(1) {
        1 => (4, 4),
        2 => (4, 2),
        3..=4 => (2, 2),
        5..=8 => (2, 1),
        _ => (1, 1),
    }
}

/// NVN2's raw storage class of a texture (the page kind its memory needs): 0 no format, 1 linear, 2 colour and
/// DEPTH32F, 3 STENCIL8 / DEPTH16, 4 DEPTH24 / DEPTH24_STENCIL8, 5 DEPTH32F_STENCIL8.
pub fn raw_storage_class(d: &crate::tex::layout::TexDesc) -> i32 {
    if d.format == fmt::NONE {
        return 0;
    }
    if d.flags as u32 & (tex_flags::LINEAR | tex_flags::LINEAR_RENDER_TARGET) != 0 {
        return 1;
    }
    match d.format {
        fmt::STENCIL8 | fmt::DEPTH16 => 3,
        fmt::DEPTH24 | fmt::DEPTH24_STENCIL8 => 4,
        fmt::DEPTH32F_STENCIL8 => 5,
        _ => 2,
    }
}

const SWZ_IDENTITY: u16 = 2 | 3 << 3 | 4 << 6 | 5 << 9;
unsafe fn put_swz(w: u16, r: *mut i32, g: *mut i32, b: *mut i32, a: *mut i32) {
    for (i, p) in [r, g, b, a].into_iter().enumerate() {
        if !p.is_null() {
            *p = (w >> (3 * i) & 7) as i32;
        }
    }
}
unsafe fn sparse(d: &crate::tex::layout::TexDesc, o: *mut NVNtextureSparseTileLayout) {
    let (n, tw, th, td) = crate::tex::layout::sparse_tiles(d);
    *o = NVNtextureSparseTileLayout {
        num_tiled_levels: n,
        tile_width: tw as i32,
        tile_height: th as i32,
        tile_depth: td as i32,
        reserved: [0; 16],
    };
}

fn switch2() -> bool {
    super::device::SWITCH2.load(Ordering::Relaxed)
}
/// The NVN2 part of a window builder (None on a Switch game, whose builder is only 0x40 bytes).
unsafe fn wb2<'a>(b: *const NVNwindowBuilder) -> Option<&'a mut win_builder2_t> {
    if switch2() {
        Some(&mut *((b as *mut u8).add(WIN_BUILDER2_OFF) as *mut win_builder2_t))
    } else {
        crate::log::red_once("an NVN2 window builder function (colour space / HDR metadata) on a Switch game's 0x40-byte builder: not stored");
        None
    }
}

/// An NVN2 acceleration-structure build description (`NVNaccelerationStructureBuildInfo`, 0x40 bytes) and its
/// geometries (0x40 bytes each).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsBuild {
    pub flags: u32,
    pub operation: u32,
    pub geometries: *const AsGeometry,
    pub count: u32,
    pub _pad: u32,
    pub source: u64,
    pub destination: u64,
    pub destination_size: u64,
    pub scratch: u64,
    pub scratch_size: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsGeometry {
    pub kind: u32,
    pub _pad: u32,
    pub w: [u64; 7],
}
const _: () =
    assert!(core::mem::size_of::<AsBuild>() == 0x40 && core::mem::size_of::<AsGeometry>() == 0x40);
pub const AS_ALLOW_UPDATE: u32 = 1;

/// Storage, build scratch and update scratch bytes of an acceleration structure. The host keeps its own acceleration
/// structures, keyed by the address the game builds at; the sizes the game allocates are an upper bound of a BVH's:
/// 192 bytes of nodes per primitive (or instance) and 128 of build scratch, plus a header, 256-aligned.
/// None for a description NVN rejects.
/// # Safety
/// `b`'s geometries are readable.
pub unsafe fn as_sizes(b: &AsBuild) -> Option<(u64, u64, u64)> {
    if (b.count != 0 && b.geometries.is_null()) || b.operation > 1 || b.flags & !15 != 0 {
        return None;
    }
    let mut prims = 0u64;
    for i in 0..b.count as usize {
        let g = &*b.geometries.add(i);
        prims += match g.kind {
            // triangles: {vertices, vertex count | format << 32, stride, indices, index count | type << 32, ...}
            1 => {
                let vcount = g.w[1] & 0xffff_ffff;
                let icount = g.w[4] & 0xffff_ffff;
                (if icount != 0 { icount } else { vcount }) / 3
            }
            // bounding boxes: {boxes, count | stride << 32, ...}; instances: {address, count, ...}
            2 | 3 => g.w[1] & 0xffff_ffff,
            _ => return None,
        };
    }
    let a = |n: u64| (n + 255) & !255;
    let storage = a(1024 + prims * 192);
    let scratch = a(512 + prims * 128);
    let update = if b.flags & AS_ALLOW_UPDATE != 0 {
        a(512 + prims * 64)
    } else {
        0
    };
    Some((storage, scratch, update))
}

nvn_fns! {
    // ---------- blend state: advanced blend ----------
    fn nvnBlendStateSetAdvancedPremultipliedSrc(s: *mut NVNblendState, pm: u32) { (*(s as *mut blend_st)).set(BF_ADV_NPM, !super::b(pm) as u32); }
    fn nvnBlendStateSetAdvancedNormalizedDst(s: *mut NVNblendState, n: u32) { (*(s as *mut blend_st)).set(BF_ADV_DST_FREE, !super::b(n) as u32); }
    fn nvnBlendStateSetAdvancedOverlap(s: *mut NVNblendState, o: i32) { (*(s as *mut blend_st)).set(BF_ADV_OVL, o as u32 & 3); }
    fn nvnBlendStateGetAdvancedMode(s: *const NVNblendState) -> i32 { (*(s as *const blend_st)).get(blend_field::ADV) as i32 }
    fn nvnBlendStateGetAdvancedPremultipliedSrc(s: *const NVNblendState) -> NVNboolean { ((*(s as *const blend_st)).get(BF_ADV_NPM) == 0) as u8 }
    fn nvnBlendStateGetAdvancedNormalizedDst(s: *const NVNblendState) -> NVNboolean { ((*(s as *const blend_st)).get(BF_ADV_DST_FREE) == 0) as u8 }
    fn nvnBlendStateGetAdvancedOverlap(s: *const NVNblendState) -> i32 { (*(s as *const blend_st)).get(BF_ADV_OVL) as i32 }

    // ---------- depth-stencil state ----------
    /// BACK reads the back face's function, any other face the front's.
    fn nvnDepthStencilStateGetStencilFunc(s: *const NVNdepthStencilState, f: i32) -> i32 {
        let o = *(s as *const ds_st);
        o.get(if f as u32 == face::BACK { ds_field::SF_B } else { ds_field::SF_F }) as i32
    }
    fn nvnDepthStencilStateGetStencilOp(s: *const NVNdepthStencilState, f: i32, fail: *mut i32, zfail: *mut i32, zpass: *mut i32) {
        let o = *(s as *const ds_st);
        let k = if f as u32 == face::BACK { [ds_field::FAIL_B, ds_field::ZFAIL_B, ds_field::ZPASS_B] } else { [ds_field::FAIL_F, ds_field::ZFAIL_F, ds_field::ZPASS_F] };
        for (p, i) in [fail, zfail, zpass].into_iter().zip(k) {
            if !p.is_null() { *p = o.get(i) as i32; }
        }
    }

    // ---------- multisample state ----------
    fn nvnMultisampleStateSetRasterSamples(s: *mut NVNmultisampleState, n: i32) { ms(s).raster_samples = n as u8; }
    fn nvnMultisampleStateSetCoverageModulationMode(s: *mut NVNmultisampleState, m: i32) { ms(s).cov_mod = (m & 3) as u8; }
    fn nvnMultisampleStateSetCoverageToColorEnable(s: *mut NVNmultisampleState, e: u32) { set_bit(&mut ms(s).bits, MS_COV2COLOR, super::b(e)); }
    fn nvnMultisampleStateSetCoverageToColorOutput(s: *mut NVNmultisampleState, i: i32) { ms(s).cov2color_out = (i & 7) as u8; }
    fn nvnMultisampleStateSetSampleLocationsEnable(s: *mut NVNmultisampleState, e: u32) { set_bit(&mut ms(s).bits, MS_SL_EN, super::b(e)); }
    fn nvnMultisampleStateSetSampleLocationsGridEnable(s: *mut NVNmultisampleState, e: u32) { set_bit(&mut ms(s).bits, MS_SL_GRID, super::b(e)); }
    fn nvnMultisampleStateSetCoverageTruncationEnable(s: *mut NVNmultisampleState, e: u32) { set_bit(&mut ms(s).bits, MS_COV_TRUNC, super::b(e)); }
    /// `xy` holds `count` (x, y) pairs for table entries `first` on; each coordinate is clamped to [0, 1] and kept in
    /// 1/16 pixel. Entries past the table's 16 are dropped.
    fn nvnMultisampleStateSetSampleLocations(s: *mut NVNmultisampleState, first: i32, count: i32, xy: *const f32) {
        if xy.is_null() || first < 0 || count <= 0 { return; }
        let o = ms(s);
        for i in 0..count as usize {
            let e = first as usize + i;
            if e >= o.sl.len() { crate::log::red_once(&format!("MultisampleStateSetSampleLocations: entry {e} past the 16-entry table")); break; }
            o.sl[e] = sl_pack(*xy.add(2 * i)) | sl_pack(*xy.add(2 * i + 1)) << 4;
        }
    }
    fn nvnMultisampleStateGetSampleLocations(s: *const NVNmultisampleState, first: i32, count: i32, xy: *mut f32) {
        if xy.is_null() || first < 0 || count <= 0 { return; }
        let o = ms(s);
        for i in 0..count as usize {
            let e = first as usize + i;
            if e >= o.sl.len() { break; }
            *xy.add(2 * i) = sl_unpack(o.sl[e]);
            *xy.add(2 * i + 1) = sl_unpack(o.sl[e] >> 4);
        }
    }
    fn nvnMultisampleStateGetSampleLocationsGrid(s: *mut NVNmultisampleState, w: *mut i32, h: *mut i32) {
        let o = ms(s);
        let (gw, gh) = sl_grid((o.samples as u32).max(o.raster_samples as u32));
        if !w.is_null() { *w = gw; }
        if !h.is_null() { *h = gh; }
    }
    fn nvnMultisampleStateGetAlphaToCoverageDither(s: *const NVNmultisampleState) -> NVNboolean { ms(s).a2c_dither }
    fn nvnMultisampleStateGetRasterSamples(s: *mut NVNmultisampleState) -> i32 { ms(s).raster_samples as i32 }
    fn nvnMultisampleStateGetCoverageModulationMode(s: *const NVNmultisampleState) -> i32 { ms(s).cov_mod as i32 }
    fn nvnMultisampleStateGetCoverageToColorEnable(s: *const NVNmultisampleState) -> NVNboolean { (ms(s).bits & MS_COV2COLOR != 0) as u8 }
    fn nvnMultisampleStateGetCoverageToColorOutput(s: *const NVNmultisampleState) -> i32 { ms(s).cov2color_out as i32 }
    fn nvnMultisampleStateGetSampleLocationsEnable(s: *const NVNmultisampleState) -> NVNboolean { (ms(s).bits & MS_SL_EN != 0) as u8 }
    fn nvnMultisampleStateGetSampleLocationsGridEnable(s: *const NVNmultisampleState) -> NVNboolean { (ms(s).bits & MS_SL_GRID != 0) as u8 }
    fn nvnMultisampleStateGetCoverageTruncationEnable(s: *const NVNmultisampleState) -> NVNboolean { (ms(s).bits & MS_COV_TRUNC != 0) as u8 }

    // ---------- samplers ----------
    // Integer border colours are kept as the float values the replay samples with (SamplerBuilderSetBorderColori/ui).
    fn nvnSamplerBuilderGetBorderColori(b: *const NVNsamplerBuilder, c: *mut i32) { let p = &(*(b as *const samp_builder_t)).p; for i in 0..4 { *c.add(i) = p.border[i] as i32; } }
    fn nvnSamplerBuilderGetBorderColorui(b: *const NVNsamplerBuilder, c: *mut u32) { let p = &(*(b as *const samp_builder_t)).p; for i in 0..4 { *c.add(i) = p.border[i] as u32; } }
    fn nvnSamplerBuilderGetLodSnap(b: *const NVNsamplerBuilder) -> f32 { (*(b as *const samp_builder_t)).lod_snap }
    fn nvnSamplerGetBorderColor(s: *const NVNsampler, c: *mut f32) { core::ptr::write_unaligned(c as *mut [f32; 4], (*(s as *const samp_t)).p.border); }
    fn nvnSamplerGetBorderColori(s: *const NVNsampler, c: *mut i32) { let p = &(*(s as *const samp_t)).p; for i in 0..4 { *c.add(i) = p.border[i] as i32; } }
    fn nvnSamplerGetBorderColorui(s: *const NVNsampler, c: *mut u32) { let p = &(*(s as *const samp_t)).p; for i in 0..4 { *c.add(i) = p.border[i] as u32; } }
    fn nvnSamplerGetCompare(s: *const NVNsampler, m: *mut i32, f: *mut i32) {
        let p = &(*(s as *const samp_t)).p;
        if !m.is_null() { *m = p.cmp_mode as i32; }
        if !f.is_null() { *f = p.cmp_func as i32; }
    }
    fn nvnSamplerGetReductionFilter(s: *const NVNsampler) -> i32 { (*(s as *const samp_t)).p.reduction as i32 }
    /// Debug IDs: the release driver's getters return 0.
    fn nvnSamplerGetDebugID(s: *const NVNsampler) -> u64 { let _ = s; 0 }
    fn nvnTextureGetDebugID(t: *const NVNtexture) -> u64 { let _ = t; 0 }
    fn nvnBufferGetDebugID(b: *const NVNbuffer) -> u64 { let _ = b; 0 }

    // ---------- texture builders, textures, views ----------
    fn nvnTextureBuilderGetSwizzle(b: *const NVNtextureBuilder, r: *mut i32, g: *mut i32, bl: *mut i32, a: *mut i32) {
        let o = &*(b as *const tex_builder_t);
        put_swz(if o.bits & 1 != 0 { o.swz } else { SWZ_IDENTITY }, r, g, bl, a);
    }
    fn nvnTextureBuilderGetDepthStencilMode(b: *const NVNtextureBuilder) -> i32 { ((*(b as *const tex_builder_t)).bits >> 1 & 1) as i32 }
    fn nvnTextureBuilderGetSparseTileLayout(b: *const NVNtextureBuilder, o: *mut NVNtextureSparseTileLayout) { sparse(&(*(b as *const tex_builder_t)).d, o); }
    fn nvnTextureBuilderGetRawStorageClass(b: *const NVNtextureBuilder) -> i32 { raw_storage_class(&(*(b as *const tex_builder_t)).d) }
    fn nvnTextureBuilderGetPackagedTextureData(b: *const NVNtextureBuilder) -> *const u8 { (*(b as *const tex_builder_t)).pkg_data }
    /// The layout's 12 bits the driver keeps (byte 0, byte 1's low nibble), the rest zero; whether one was set.
    fn nvnTextureBuilderGetPackagedTextureLayout(b: *const NVNtextureBuilder, l: *mut NVNpackagedTextureLayout) -> NVNboolean {
        let o = &*(b as *const tex_builder_t);
        if !l.is_null() {
            let mut v = [0u8; 8];
            if o.has_layout != 0 { v[0] = o.layout[0]; v[1] = o.layout[1] & 0xf; }
            (*l).layout = v;
        }
        o.has_layout
    }
    fn nvnTextureBuilderSetGLTextureName(b: *mut NVNtextureBuilder, n: u32) {
        if n != 0 { crate::log::red_once(&format!("TextureBuilderSetGLTextureName {n}: the host has no GL interop; the texture keeps its own storage")); }
        (*(b as *mut tex_builder_t)).gl_name = n;
    }
    fn nvnTextureBuilderGetGLTextureName(b: *const NVNtextureBuilder) -> u32 { (*(b as *const tex_builder_t)).gl_name }
    fn nvnTextureGetSwizzle(t: *const NVNtexture, r: *mut i32, g: *mut i32, bl: *mut i32, a: *mut i32) { put_swz((*(t as *const tex_t)).swizzle(), r, g, bl, a); }
    fn nvnTextureGetDepthStencilMode(t: *const NVNtexture) -> i32 { ((*(t as *const tex_t)).bits >> 1 & 1) as i32 }
    fn nvnTextureGetSparseTileLayout(t: *const NVNtexture, o: *mut NVNtextureSparseTileLayout) { sparse(&(*(t as *const tex_t)).d, o); }
    fn nvnTextureGetRawStorageClass(t: *const NVNtexture) -> i32 { raw_storage_class(&(*(t as *const tex_t)).d) }
    /// The channels (identity R G B A when SetSwizzle was not called) and whether the view sets them.
    fn nvnTextureViewGetSwizzle(v: *const NVNtextureView, r: *mut i32, g: *mut i32, bl: *mut i32, a: *mut i32) -> NVNboolean {
        let o = &*(v as *const view_t);
        let set = o.bits & VW_SWIZZLE != 0;
        for (c, p) in [r, g, bl, a].into_iter().enumerate() {
            if !p.is_null() { *p = if set { o.swz[c] as i32 } else { swizzle::R as i32 + c as i32 }; }
        }
        set as u8
    }
    fn nvnTextureViewGetDepthStencilMode(v: *const NVNtextureView, m: *mut i32) -> NVNboolean {
        let o = &*(v as *const view_t);
        if !m.is_null() { *m = (o.bits >> 6 & 3) as i32; }
        (o.bits & VW_DSMODE != 0) as u8
    }

    // ---------- queue builders and queues ----------
    fn nvnQueueBuilderGetDevice(b: *const NVNqueueBuilder) -> *mut NVNdevice { (*(b as *const queue_builder_t)).dev }
    fn nvnQueueBuilderGetFlags(b: *const NVNqueueBuilder) -> i32 { (*(b as *const queue_builder_t)).flags as i32 }
    fn nvnQueueBuilderGetCommandFlushThreshold(b: *const NVNqueueBuilder, n: *mut i32) -> NVNboolean {
        let o = &*(b as *const queue_builder_t);
        if !n.is_null() { *n = o.flush as i32; }
        o.flush_set
    }
    fn nvnQueueBuilderGetMemory(b: *const NVNqueueBuilder) -> *mut u8 { (*(b as *const queue_builder_t)).mem }
    fn nvnQueueBuilderGetMemorySize(b: *const NVNqueueBuilder) -> i32 { (*(b as *const queue_builder_t)).mem_sz as i32 }
    fn nvnQueueBuilderSetRayTracingMemorySize(b: *mut NVNqueueBuilder, n: usize) { let o = &mut *(b as *mut queue_builder_t); o.sz[QB_RT] = n as u32; o.set[QB_RT] = 1; }
    fn nvnQueueBuilderGetRayTracingMemorySize(b: *const NVNqueueBuilder, n: *mut i32) -> NVNboolean {
        let o = &*(b as *const queue_builder_t);
        if !n.is_null() { *n = o.sz[QB_RT] as i32; }
        o.set[QB_RT]
    }
    /// The host runs every queue's submissions in order on one Vulkan queue: the priority and the primary queue of a
    /// subcontext queue are kept for their getters.
    fn nvnQueueBuilderSetQueuePriority(b: *mut NVNqueueBuilder, p: i32) { (*(b as *mut queue_builder_t)).prio = p; }
    fn nvnQueueBuilderGetQueuePriority(b: *const NVNqueueBuilder) -> i32 { (*(b as *const queue_builder_t)).prio }
    fn nvnQueueBuilderSetPrimaryQueue(b: *mut NVNqueueBuilder, q: *mut NVNqueue) { (*(b as *mut queue_builder_t)).primary = q; }
    fn nvnQueueBuilderGetPrimaryQueue(b: *const NVNqueueBuilder) -> *mut NVNqueue { (*(b as *const queue_builder_t)).primary }
    /// Queue memory usage: the host's queues replay from the game's command buffers and use none of the queue memory.
    fn nvnQueueGetTotalCommandMemoryUsed(q: *mut NVNqueue) -> usize { let _ = q; 0 }
    fn nvnQueueGetTotalComputeMemoryUsed(q: *mut NVNqueue) -> usize { let _ = q; 0 }
    fn nvnQueueGetTotalControlMemoryUsed(q: *mut NVNqueue) -> usize { let _ = q; 0 }
    fn nvnQueueGetTotalRayTracingMemoryUsed(q: *mut NVNqueue) -> usize { let _ = q; 0 }
    fn nvnQueueResetMemoryUsageCounts(q: *mut NVNqueue) { let _ = q; }

    // ---------- window builders and windows ----------
    fn nvnWindowBuilderGetDevice(b: *const NVNwindowBuilder) -> *mut NVNdevice { (*(b as *const win_builder_t)).dev }
    fn nvnWindowBuilderGetNumTextures(b: *const NVNwindowBuilder) -> i32 { (*(b as *const win_builder_t)).ntex }
    fn nvnWindowBuilderGetTexture(b: *const NVNwindowBuilder, i: i32) -> *mut NVNtexture {
        let o = &*(b as *const win_builder_t);
        if i >= 0 && i < o.ntex { o.tex[i as usize] } else { core::ptr::null_mut() }
    }
    /// The builder's own value: 0 = every texture.
    fn nvnWindowBuilderGetNumActiveTextures(b: *const NVNwindowBuilder) -> i32 { (*(b as *const win_builder_t)).nactive }
    /// Kept for the getter: nvnWindowInitialize does not read it (as the driver).
    fn nvnWindowBuilderSetColorSpace(b: *mut NVNwindowBuilder, c: i32) { if let Some(x) = wb2(b) { x.color_space = c; } }
    fn nvnWindowBuilderGetColorSpace(b: *const NVNwindowBuilder) -> i32 { wb2(b).map_or(0, |x| x.color_space) }
    fn nvnWindowBuilderSetHdrDisplayMetadata(b: *mut NVNwindowBuilder, a0: f32, a1: f32, a2: f32, a3: f32, a4: f32, a5: f32, a6: f32, a7: f32, a8: f32, a9: f32) {
        if let Some(x) = wb2(b) { x.hdr_disp = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9]; }
    }
    fn nvnWindowBuilderGetHdrDisplayMetadata(b: *const NVNwindowBuilder, o0: *mut f32, o1: *mut f32, o2: *mut f32, o3: *mut f32, o4: *mut f32, o5: *mut f32, o6: *mut f32, o7: *mut f32, o8: *mut f32, o9: *mut f32) {
        let v = wb2(b).map_or(HDR_DISPLAY_DEFAULT, |x| x.hdr_disp);
        for (p, x) in [o0, o1, o2, o3, o4, o5, o6, o7, o8, o9].into_iter().zip(v) { if !p.is_null() { *p = x; } }
    }
    fn nvnWindowBuilderSetHdrContentMetadata(b: *mut NVNwindowBuilder, a0: f32, a1: f32) { if let Some(x) = wb2(b) { x.hdr_content = [a0, a1]; } }
    fn nvnWindowBuilderGetHdrContentMetadata(b: *const NVNwindowBuilder, o0: *mut f32, o1: *mut f32) {
        let v = wb2(b).map_or(HDR_CONTENT_DEFAULT, |x| x.hdr_content);
        if !o0.is_null() { *o0 = v[0]; }
        if !o1.is_null() { *o1 = v[1]; }
    }
    /// The window's HDR metadata: stored; the system's display path does not take it from NVN (the driver forwards it
    /// to a no-op), and neither does the host's.
    fn nvnWindowSetHdrDisplayMetadata(w: *mut NVNwindow, a0: f32, a1: f32, a2: f32, a3: f32, a4: f32, a5: f32, a6: f32, a7: f32, a8: f32, a9: f32) {
        (*(w as *mut win_t)).hdr_disp = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9];
    }
    fn nvnWindowGetHdrDisplayMetadata(w: *const NVNwindow, o0: *mut f32, o1: *mut f32, o2: *mut f32, o3: *mut f32, o4: *mut f32, o5: *mut f32, o6: *mut f32, o7: *mut f32, o8: *mut f32, o9: *mut f32) {
        let v = (*(w as *const win_t)).hdr_disp;
        for (p, x) in [o0, o1, o2, o3, o4, o5, o6, o7, o8, o9].into_iter().zip(v) { if !p.is_null() { *p = x; } }
    }
    fn nvnWindowSetHdrContentMetadata(w: *mut NVNwindow, a0: f32, a1: f32) { (*(w as *mut win_t)).hdr_content = [a0, a1]; }
    fn nvnWindowGetHdrContentMetadata(w: *const NVNwindow, o0: *mut f32, o1: *mut f32) {
        let v = (*(w as *const win_t)).hdr_content;
        if !o0.is_null() { *o0 = v[0]; }
        if !o1.is_null() { *o1 = v[1]; }
    }

    // ---------- event builders ----------
    fn nvnEventBuilderGetStorage(b: *const NVNeventBuilder, off: *mut i64) -> *const NVNmemoryPool {
        let o = &*(b as *const ev_builder_t);
        if !off.is_null() { *off = o.off; }
        o.pool as *const NVNmemoryPool
    }
    fn nvnEventBuilderGetMemoryPool(b: *const NVNeventBuilder) -> *const NVNmemoryPool { (*(b as *const ev_builder_t)).pool as *const NVNmemoryPool }
    fn nvnEventBuilderGetMemoryOffset(b: *const NVNeventBuilder) -> i64 { (*(b as *const ev_builder_t)).off }

    // ---------- device ----------
    /// The builder's flags.
    fn nvnDeviceGetFlags(d: *const NVNdevice) -> i32 { (*(d as *const dev_t)).flags as i32 }
    /// NVN2's bindless uniform buffer handle: (address >> 4) | ceil(size / 16) << 45, the size field capped at
    /// 2^19 - 1 (`device::ucb_handle`).
    fn nvnDeviceGetUniformBufferHandle(d: *const NVNdevice, addr: u64, size: u64) -> u64 { let _ = d; super::device::ucb_handle(addr, size) }
    /// The command memory size a direct (tag 2) handle encodes; the host's handles are control-list addresses (tag 0).
    fn nvnDeviceGetCommandHandleMemorySize(d: *const NVNdevice, h: u64) -> u64 { let _ = d; if h & 3 == 2 { (h >> 38) & 0x3ff_fffc } else { 0 } }
    fn nvnDeviceIsExternalDebuggerAttached(d: *const NVNdevice) -> NVNboolean { let _ = d; 0 }
    fn nvnSetExternalDebuggerEnabledNVX(on: u32) { let _ = on; }
    /// The release driver's debug database walk visits nothing.
    fn nvnDeviceWalkDebugDatabase(d: *const NVNdevice, kind: i32, cb: u64, data: u64) { let _ = (d, kind, cb, data); }
    // Checkpoints (GPU crash breadcrumbs): the release driver's are no-ops that answer 1.
    fn nvnDeviceSetAutomaticCheckpointsEnable(d: *mut NVNdevice, v: u64) -> u32 { let _ = (d, v); 1 }
    fn nvnDeviceSetAutomaticCheckpointsMask(d: *mut NVNdevice, v: u64) -> u32 { let _ = (d, v); 1 }
    fn nvnDeviceSetAutomaticCheckpointsSamplingInterval(d: *mut NVNdevice, v: u64) -> u32 { let _ = (d, v); 1 }
    fn nvnDeviceSetAutomaticCheckpointsByDebugGroup(d: *mut NVNdevice, v: u64) -> u32 { let _ = (d, v); 1 }
    fn nvnDeviceSetCheckpointCallStacksEnable(d: *mut NVNdevice, v: u64) -> u32 { let _ = (d, v); 1 }
    fn nvnDeviceSetCheckpointMaxCallStacksDepth(d: *mut NVNdevice, v: u64) -> u32 { let _ = (d, v); 1 }
    // Fast-clear values are a compression hint of the Tegra's: the host's images keep their own clear representation,
    // and the cleared contents come from the recorded clears. Every registration is accepted.
    fn nvnDeviceRegisterFastClearStencil(d: *mut NVNdevice, v: i32) -> NVNboolean { let _ = (d, v); 1 }
    fn nvnDeviceRegisterFastClearColorIndex(d: *mut NVNdevice, c: *const f32, f: i32, i: i32) { let _ = (d, c, f, i); }
    fn nvnDeviceRegisterFastClearColoriIndex(d: *mut NVNdevice, c: *const i32, f: i32, i: i32) { let _ = (d, c, f, i); }
    fn nvnDeviceRegisterFastClearColoruiIndex(d: *mut NVNdevice, c: *const u32, f: i32, i: i32) { let _ = (d, c, f, i); }
    fn nvnDeviceRegisterFastClearDepthIndex(d: *mut NVNdevice, v: f32, i: i32) { let _ = (d, v, i); }
    fn nvnDeviceRegisterFastClearStencilIndex(d: *mut NVNdevice, v: i32, i: i32) { let _ = (d, v, i); }
    /// Acceleration structure sizes (`as_sizes`); a description NVN rejects gets 0s and a red line.
    fn nvnDeviceCalculateAccelerationStructureMemoryRequirements(d: *const NVNdevice, b: *const AsBuild, storage: *mut u64, scratch: *mut u64, update: *mut u64) {
        let _ = d;
        let r = if b.is_null() { None } else { as_sizes(&*b) };
        if r.is_none() { crate::log::red("DeviceCalculateAccelerationStructureMemoryRequirements: an invalid build description"); }
        let (s, c, u) = r.unwrap_or((0, 0, 0));
        if !storage.is_null() { *storage = s; }
        if !scratch.is_null() { *scratch = c; }
        if !update.is_null() { *update = u; }
    }

    // ---------- GL sync interop ----------
    // Only through the GL interop interface nvnSetGLInteropNVX installs, which the host does not have: the driver's
    // answers without one (error 4, nothing initialized).
    fn nvnSyncInitializeFromFencedGLSync(s: *mut NVNsync, d: *mut NVNdevice, gl: u64, err: *mut i32) -> NVNboolean {
        let _ = (s, d, gl);
        if !err.is_null() { *err = 4; }
        0
    }
    fn nvnSyncCreateGLSync(s: *const NVNsync, gl: *mut u64) -> i32 { let _ = (s, gl); 4 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::state::*;

    #[test]
    fn advanced_blend_and_stencil_getters() {
        let mut b = NVNblendState::boxed();
        let p = &mut *b as *mut NVNblendState;
        unsafe {
            nvnBlendStateSetDefaults(p);
            assert_eq!(nvnBlendStateGetAdvancedPremultipliedSrc(p), 1);
            assert_eq!(nvnBlendStateGetAdvancedNormalizedDst(p), 1);
            nvnBlendStateSetAdvancedMode(p, 8);
            nvnBlendStateSetAdvancedPremultipliedSrc(p, 0x100); // only the low 8 bits count: false
            nvnBlendStateSetAdvancedOverlap(p, 2);
            nvnBlendStateSetAdvancedNormalizedDst(p, 0);
            assert_eq!(
                (
                    nvnBlendStateGetAdvancedMode(p),
                    nvnBlendStateGetAdvancedPremultipliedSrc(p),
                    nvnBlendStateGetAdvancedOverlap(p),
                    nvnBlendStateGetAdvancedNormalizedDst(p)
                ),
                (8, 0, 2, 0)
            );
        }
        let mut d = NVNdepthStencilState::boxed();
        let p = &mut *d as *mut NVNdepthStencilState;
        unsafe {
            nvnDepthStencilStateSetDefaults(p);
            nvnDepthStencilStateSetStencilFunc(p, face::BACK as i32, cmp::EQUAL as i32);
            nvnDepthStencilStateSetStencilOp(
                p,
                face::FRONT as i32,
                stencil_op::ZERO as i32,
                stencil_op::INCR as i32,
                stencil_op::REPLACE as i32,
            );
            assert_eq!(
                nvnDepthStencilStateGetStencilFunc(p, face::BACK as i32),
                cmp::EQUAL as i32
            );
            assert_eq!(
                nvnDepthStencilStateGetStencilFunc(p, face::FRONT as i32),
                cmp::ALWAYS as i32
            );
            let (mut a, mut z, mut s) = (0, 0, 0);
            nvnDepthStencilStateGetStencilOp(p, face::FRONT as i32, &mut a, &mut z, &mut s);
            assert_eq!(
                (a, z, s),
                (
                    stencil_op::ZERO as i32,
                    stencil_op::INCR as i32,
                    stencil_op::REPLACE as i32
                )
            );
            nvnDepthStencilStateGetStencilOp(p, face::BACK as i32, &mut a, &mut z, &mut s);
            assert_eq!(a, stencil_op::KEEP as i32);
        }
    }

    #[test]
    fn multisample_state_round_trips() {
        let mut m = NVNmultisampleState::boxed();
        let p = &mut *m as *mut NVNmultisampleState;
        unsafe {
            nvnMultisampleStateSetDefaults(p);
            let mut xy = [0f32; 4];
            nvnMultisampleStateGetSampleLocations(p, 0, 2, xy.as_mut_ptr());
            assert_eq!(xy, [0.5; 4]);
            nvnMultisampleStateSetSampleLocations(p, 3, 2, [0.25f32, 1.5, -1.0, 0.75].as_ptr());
            nvnMultisampleStateGetSampleLocations(p, 3, 2, xy.as_mut_ptr());
            assert_eq!(xy, [0.25, 0.9375, 0.0, 0.75]);
            nvnMultisampleStateSetSamples(p, 4);
            nvnMultisampleStateSetRasterSamples(p, 8);
            nvnMultisampleStateSetCoverageToColorEnable(p, 1);
            nvnMultisampleStateSetCoverageToColorOutput(p, 5);
            nvnMultisampleStateSetCoverageTruncationEnable(p, 1);
            nvnMultisampleStateSetSampleLocationsGridEnable(p, 1);
            nvnMultisampleStateSetCoverageModulationMode(p, 3);
            let (mut w, mut h) = (0, 0);
            nvnMultisampleStateGetSampleLocationsGrid(p, &mut w, &mut h);
            assert_eq!((w, h), (2, 1));
            assert_eq!(nvnMultisampleStateGetRasterSamples(p), 8);
            assert_eq!(
                (
                    nvnMultisampleStateGetCoverageToColorEnable(p),
                    nvnMultisampleStateGetCoverageToColorOutput(p)
                ),
                (1, 5)
            );
            assert_eq!(
                (
                    nvnMultisampleStateGetCoverageTruncationEnable(p),
                    nvnMultisampleStateGetSampleLocationsGridEnable(p),
                    nvnMultisampleStateGetSampleLocationsEnable(p)
                ),
                (1, 1, 0)
            );
            assert_eq!(nvnMultisampleStateGetCoverageModulationMode(p), 3);
            assert_eq!(nvnMultisampleStateGetAlphaToCoverageDither(p), 1);
            // the bound bytes stay within the state object
            assert_eq!(nvnMultisampleStateGetSamples(p), 4);
        }
    }

    #[test]
    fn raw_storage_classes_and_view_getters() {
        let mut d = crate::tex::layout::TexDesc {
            format: fmt::RGBA8,
            ..Default::default()
        };
        assert_eq!(raw_storage_class(&d), 2);
        d.flags = tex_flags::LINEAR as i32;
        assert_eq!(raw_storage_class(&d), 1);
        d.flags = 0;
        for (f, c) in [
            (fmt::STENCIL8, 3),
            (fmt::DEPTH16, 3),
            (fmt::DEPTH24_STENCIL8, 4),
            (fmt::DEPTH32F, 2),
            (fmt::DEPTH32F_STENCIL8, 5),
            (fmt::NONE, 0),
        ] {
            d.format = f;
            assert_eq!(raw_storage_class(&d), c, "format {f}");
        }
        let mut v = NVNtextureView::boxed();
        let p = &mut *v as *mut NVNtextureView;
        let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
        unsafe {
            crate::nvn::tex::nvnTextureViewSetDefaults(p);
            assert_eq!(
                nvnTextureViewGetSwizzle(p, &mut r, &mut g, &mut b, &mut a),
                0
            );
            assert_eq!((r, g, b, a), (2, 3, 4, 5));
            crate::nvn::tex::nvnTextureViewSetSwizzle(p, 5, 4, 3, 2);
            crate::nvn::tex::nvnTextureViewSetDepthStencilMode(p, 1);
            assert_eq!(
                nvnTextureViewGetSwizzle(p, &mut r, &mut g, &mut b, &mut a),
                1
            );
            assert_eq!((r, g, b, a), (5, 4, 3, 2));
            let mut m = 0;
            assert_eq!(nvnTextureViewGetDepthStencilMode(p, &mut m), 1);
            assert_eq!(m, 1);
        }
    }

    #[test]
    fn queue_builder_and_handles() {
        let mut q = NVNqueueBuilder::boxed();
        let p = &mut *q as *mut NVNqueueBuilder;
        unsafe {
            crate::nvn::queue::nvnQueueBuilderSetDefaults(p);
            assert_eq!(nvnQueueBuilderGetQueuePriority(p), 100);
            let mut n = 0;
            assert_eq!(nvnQueueBuilderGetCommandFlushThreshold(p, &mut n), 0);
            crate::nvn::queue::nvnQueueBuilderSetCommandFlushThreshold(p, 0x4000);
            nvnQueueBuilderSetRayTracingMemorySize(p, 0x2000);
            nvnQueueBuilderSetQueuePriority(p, 1000);
            assert_eq!(nvnQueueBuilderGetCommandFlushThreshold(p, &mut n), 1);
            assert_eq!(n, 0x4000);
            assert_eq!(nvnQueueBuilderGetRayTracingMemorySize(p, &mut n), 1);
            assert_eq!((n, nvnQueueBuilderGetQueuePriority(p)), (0x2000, 1000));
        }
        unsafe {
            assert_eq!(
                nvnDeviceGetUniformBufferHandle(core::ptr::null(), 0x1_2340, 17),
                0x1234 | 2 << 45
            );
            assert_eq!(
                nvnDeviceGetCommandHandleMemorySize(core::ptr::null(), 0x1000),
                0
            );
            assert_eq!(
                nvnDeviceGetCommandHandleMemorySize(core::ptr::null(), (0x400u64 << 38) | 2),
                0x400
            );
        }
    }

    #[test]
    fn window_hdr_metadata_through_the_bridge() {
        // ten floats: eight in v0-v7, the ninth and tenth on the stack
        let mut w = NVNwindow::boxed();
        let mut stack = [0u64; 2];
        stack[0] = 0.03f32.to_bits() as u64;
        stack[1] = 1000.0f32.to_bits() as u64;
        let mut c = nxrt::cpu::Cpu::new_boxed();
        c.x[0] = w.0.as_mut_ptr() as u64;
        for (i, v) in [0.64f32, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.329]
            .into_iter()
            .enumerate()
        {
            c.v[i] = v.to_bits() as u128;
        }
        c.sp = stack.as_mut_ptr() as u64;
        unsafe { crate::nvn::find("nvnWindowSetHdrDisplayMetadata").unwrap()(&mut *c) };
        let mut o = [0f32; 10];
        let p: Vec<*mut f32> = o.iter_mut().map(|x| x as *mut f32).collect();
        unsafe {
            nvnWindowGetHdrDisplayMetadata(
                &*w, p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7], p[8], p[9],
            )
        };
        assert_eq!(o, HDR_DISPLAY_DEFAULT);
    }

    #[test]
    fn acceleration_structure_sizes() {
        let tri = AsGeometry {
            kind: 1,
            _pad: 0,
            w: [0x1000, 300, 12, 0, 0, 0, 0],
        };
        let b = AsBuild {
            flags: AS_ALLOW_UPDATE,
            operation: 0,
            geometries: &tri,
            count: 1,
            _pad: 0,
            source: 0,
            destination: 0,
            destination_size: 0,
            scratch: 0,
            scratch_size: 0,
        };
        let (s, c, u) = unsafe { as_sizes(&b) }.unwrap();
        assert!(s >= 100 * 192 && s % 256 == 0 && c >= 100 * 128 && u > 0);
        let bad = AsBuild { flags: 16, ..b };
        assert!(unsafe { as_sizes(&bad) }.is_none());
    }
}

//! Textures, texture views and samplers.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use crate::tex::layout::{self, TexDesc};
use crate::vk;

unsafe fn tb<'a>(b: *mut NVNtextureBuilder) -> &'a mut tex_builder_t {
    &mut *(b as *mut tex_builder_t)
}
unsafe fn ctb<'a>(b: *const NVNtextureBuilder) -> &'a tex_builder_t {
    &*(b as *const tex_builder_t)
}
unsafe fn ctx<'a>(t: *const NVNtexture) -> &'a tex_t {
    &*(t as *const tex_t)
}
unsafe fn vw<'a>(v: *mut NVNtextureView) -> &'a mut view_t {
    &mut *(v as *mut view_t)
}
unsafe fn sb<'a>(b: *mut NVNsamplerBuilder) -> &'a mut samp_par {
    &mut (*(b as *mut samp_builder_t)).p
}
unsafe fn csb<'a>(b: *const NVNsamplerBuilder) -> &'a samp_par {
    &(*(b as *const samp_builder_t)).p
}

fn view_offset(d: &TexDesc, layer_size: u64, w: Option<&view_t>) -> isize {
    let Some(w) = w else { return 0 };
    let mut off = if w.bits & VW_LEVELS != 0 {
        layout::level_offset(d, w.base_level as u32)
    } else {
        0
    };
    if w.bits & VW_LAYERS != 0 {
        off += w.min_layer as u64 * layer_size;
    }
    off as isize
}

/// Bytes per row and per image of a region in the texture's format, as ReadTexels / WriteTexels lay them out.
fn region_pitch(format: i32, r: &NVNcopyRegion) -> (usize, usize) {
    let (bpb, bw, bh) = layout::fmt_bpb(format);
    let row = (r.width.max(0) as u32).div_ceil(bw) as usize * bpb as usize;
    let rows = (r.height.max(0) as u32).div_ceil(bh) as usize;
    (row, row * rows)
}

nvn_fns! {
    // ---------- texture builder ----------
    fn nvnTextureBuilderSetDefaults(b: *mut NVNtextureBuilder) {
        let dev = tb(b).dev;
        core::ptr::write_bytes(b as *mut u8, 0, core::mem::size_of::<tex_builder_t>());
        tb(b).dev = dev;
        tb(b).d.target = target::T2D;
        tb(b).d.levels = 1;
    }
    fn nvnTextureBuilderSetDevice(b: *mut NVNtextureBuilder, d: *mut NVNdevice) { tb(b).dev = d; }
    fn nvnTextureBuilderGetDevice(b: *const NVNtextureBuilder) -> *mut NVNdevice { ctb(b).dev }
    fn nvnTextureBuilderSetFlags(b: *mut NVNtextureBuilder, f: i32) { tb(b).d.flags = f; }
    fn nvnTextureBuilderSetTarget(b: *mut NVNtextureBuilder, t: i32) { tb(b).d.target = t; }
    fn nvnTextureBuilderSetWidth(b: *mut NVNtextureBuilder, v: i32) { tb(b).d.width = v as u32; }
    fn nvnTextureBuilderSetHeight(b: *mut NVNtextureBuilder, v: i32) { tb(b).d.height = v as u32; }
    fn nvnTextureBuilderSetDepth(b: *mut NVNtextureBuilder, v: i32) { tb(b).d.depth = v as u32; }
    fn nvnTextureBuilderSetSize1D(b: *mut NVNtextureBuilder, w: i32) { let o = tb(b); o.d.width = w as u32; o.d.height = 1; o.d.depth = 1; }
    fn nvnTextureBuilderSetSize2D(b: *mut NVNtextureBuilder, w: i32, h: i32) { let o = tb(b); o.d.width = w as u32; o.d.height = h as u32; o.d.depth = 1; }
    fn nvnTextureBuilderSetSize3D(b: *mut NVNtextureBuilder, w: i32, h: i32, d: i32) { let o = tb(b); o.d.width = w as u32; o.d.height = h as u32; o.d.depth = d as u32; }
    fn nvnTextureBuilderSetLevels(b: *mut NVNtextureBuilder, v: i32) { tb(b).d.levels = v as u32; }
    fn nvnTextureBuilderSetFormat(b: *mut NVNtextureBuilder, f: i32) { tb(b).d.format = f; }
    fn nvnTextureBuilderSetSamples(b: *mut NVNtextureBuilder, s: i32) { tb(b).d.samples = s; }
    fn nvnTextureBuilderSetStride(b: *mut NVNtextureBuilder, s: isize) { tb(b).d.stride = s as u32; }
    fn nvnTextureBuilderSetStorage(b: *mut NVNtextureBuilder, p: *mut NVNmemoryPool, off: isize) { tb(b).pool = p as *mut pool_t; tb(b).off = off as i64; }
    fn nvnTextureBuilderSetPackagedTextureLayout(b: *mut NVNtextureBuilder, l: *const NVNpackagedTextureLayout) {
        let o = tb(b);
        o.has_layout = (!l.is_null()) as u8;
        if l.is_null() {
            o.d.pkg_layout = 0;
        } else {
            o.layout = (*l).layout;
            o.d.pkg_layout = ((*l).layout[0] as i32 | (((*l).layout[1] & 0xf) as i32) << 8) + 1;
        }
    }
    fn nvnTextureBuilderSetPackagedTextureData(b: *mut NVNtextureBuilder, data: *const u8) { tb(b).pkg_data = data; }
    fn nvnTextureBuilderSetSwizzle(b: *mut NVNtextureBuilder, r: i32, g: i32, bl: i32, a: i32) {
        let o = tb(b);
        o.bits |= 1;
        o.swz = (r as u16 & 7) | (g as u16 & 7) << 3 | (bl as u16 & 7) << 6 | (a as u16 & 7) << 9;
    }
    fn nvnTextureBuilderSetDepthStencilMode(b: *mut NVNtextureBuilder, m: i32) {
        let o = tb(b);
        if m == 1 { o.bits |= 2 } else { o.bits &= !2 }
    }
    fn nvnTextureBuilderGetFlags(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.flags }
    fn nvnTextureBuilderGetTarget(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.target }
    fn nvnTextureBuilderGetWidth(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.width as i32 }
    fn nvnTextureBuilderGetHeight(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.height as i32 }
    fn nvnTextureBuilderGetDepth(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.depth as i32 }
    fn nvnTextureBuilderGetLevels(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.levels as i32 }
    fn nvnTextureBuilderGetFormat(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.format }
    fn nvnTextureBuilderGetSamples(b: *const NVNtextureBuilder) -> i32 { ctb(b).d.samples }
    fn nvnTextureBuilderGetStride(b: *const NVNtextureBuilder) -> isize { ctb(b).d.stride as isize }
    fn nvnTextureBuilderGetMemoryPool(b: *const NVNtextureBuilder) -> *mut NVNmemoryPool { ctb(b).pool as *mut NVNmemoryPool }
    fn nvnTextureBuilderGetMemoryOffset(b: *const NVNtextureBuilder) -> isize { ctb(b).off as isize }
    fn nvnTextureBuilderGetStorageSize(b: *const NVNtextureBuilder) -> usize {
        let l = layout::tex_layout(&ctb(b).d);
        if l.bpb == 0 { crate::log::red_once(&format!("TextureBuilderGetStorageSize: unknown format {:#x}", ctb(b).d.format)); }
        l.size as usize
    }
    fn nvnTextureBuilderGetZCullStorageSize(b: *const NVNtextureBuilder) -> usize { layout::zcull_size(&ctb(b).d) as usize }
    fn nvnTextureBuilderGetStorageClass(b: *const NVNtextureBuilder) -> i32 { let _ = b; 0 }
    fn nvnTextureBuilderGetStorageAlignment(b: *const NVNtextureBuilder) -> usize { layout::tex_layout(&ctb(b).d).align as usize }
    fn nvnTextureBuilderGetViewOffset(b: *const NVNtextureBuilder, v: *const NVNtextureView) -> isize {
        let d = &ctb(b).d;
        let l = layout::tex_layout(d);
        view_offset(d, l.layer_size, (v as *const view_t).as_ref())
    }

    // ---------- textures ----------
    fn nvnTextureInitialize(t: *mut NVNtexture, b: *const NVNtextureBuilder) -> NVNboolean {
        let s = ctb(b);
        let o = t as *mut tex_t;
        // Initialize on a live texture object without a Finalize (games re-initialize transient render targets every
        // frame): the old host image goes first.
        if (*o).magic == MAGIC_TEX && (*o).serial != 0 {
            vk::Gpu::with(|g| g.tex_release((*o).serial));
        }
        core::ptr::write_bytes(o as *mut u8, 0, core::mem::size_of::<tex_t>());
        let o = &mut *o;
        o.magic = MAGIC_TEX;
        o.d = s.d;
        o.pool = s.pool;
        o.off = s.off;
        o.has_layout = s.has_layout;
        o.layout = s.layout;
        o.bits = s.bits;
        o.swz = s.swz;
        o.l = layout::tex_layout(&o.d);
        o.mem = if !s.pool.is_null() && !(*s.pool).mem.is_null() { (*s.pool).mem.add(s.off as usize) } else { core::ptr::null_mut() };
        o.serial = super::new_serial();
        // Packaged texture data is copied into the storage at Initialize.
        if !s.pkg_data.is_null() && !o.mem.is_null() {
            core::ptr::copy_nonoverlapping(s.pkg_data, o.mem, o.l.size as usize);
            vk::mem::cpu_flushed(o.mem as u64, o.l.size);
        }
        1
    }
    fn nvnTextureFinalize(t: *mut NVNtexture) {
        let o = &mut *(t as *mut tex_t);
        if o.magic == MAGIC_TEX { vk::Gpu::with(|g| g.tex_release(o.serial)); }
        o.magic = 0;
    }
    fn nvnTextureSetDebugLabel(t: *mut NVNtexture, s: *const u8) { let _ = (t, s); }
    fn nvnTextureGetFlags(t: *const NVNtexture) -> i32 { ctx(t).d.flags }
    fn nvnTextureGetTarget(t: *const NVNtexture) -> i32 { ctx(t).d.target }
    fn nvnTextureGetWidth(t: *const NVNtexture) -> i32 { ctx(t).d.width as i32 }
    fn nvnTextureGetHeight(t: *const NVNtexture) -> i32 { ctx(t).d.height as i32 }
    fn nvnTextureGetDepth(t: *const NVNtexture) -> i32 { ctx(t).d.depth as i32 }
    fn nvnTextureGetLevels(t: *const NVNtexture) -> i32 { ctx(t).d.levels as i32 }
    fn nvnTextureGetFormat(t: *const NVNtexture) -> i32 { ctx(t).d.format }
    fn nvnTextureGetSamples(t: *const NVNtexture) -> i32 { ctx(t).d.samples }
    fn nvnTextureGetStride(t: *const NVNtexture) -> isize { ctx(t).d.stride as isize }
    fn nvnTextureGetStorageSize(t: *const NVNtexture) -> i32 { ctx(t).l.size as i32 }
    fn nvnTextureGetMemoryPool(t: *const NVNtexture) -> *mut NVNmemoryPool { ctx(t).pool as *mut NVNmemoryPool }
    fn nvnTextureGetMemoryOffset(t: *const NVNtexture) -> isize { ctx(t).off as isize }
    fn nvnTextureGetTextureAddress(t: *const NVNtexture) -> u64 { let o = ctx(t); if o.pool.is_null() { 0 } else { (*o.pool).mem as u64 + o.off as u64 } }
    fn nvnTextureGetZCullStorageSize(t: *const NVNtexture) -> usize { layout::zcull_size(&ctx(t).d) as usize }
    /// Storage class: NVN's compression class of the texture's memory; every texture here is class 0 (none), as the
    /// host decides its own placement.
    fn nvnTextureGetStorageClass(t: *const NVNtexture) -> i32 { let _ = t; 0 }
    fn nvnTextureGetViewOffset(t: *const NVNtexture, v: *const NVNtextureView) -> isize {
        let o = ctx(t);
        view_offset(&o.d, o.l.layer_size, (v as *const view_t).as_ref())
    }
    fn nvnTextureCompare(a: *const NVNtexture, bb: *const NVNtexture) -> NVNboolean {
        let (x, y) = (ctx(a), ctx(bb));
        (x.d == y.d && x.mem == y.mem && x.swizzle() == y.swizzle()) as u8
    }
    // ReadTexels / WriteTexels: tightly packed rows of the region (or the strides given), in the texture's format.
    fn nvnTextureWriteTexels(t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, data: *const u8) {
        let o = *ctx(t);
        let (row, img) = region_pitch(o.d.format, &*r);
        vk::Gpu::with(|g| g.write_texels(&o, (v as *const view_t).as_ref().copied(), &*r, data, row, img));
    }
    fn nvnTextureWriteTexelsStrided(t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, data: *const u8, row: isize, img: isize) {
        let o = *ctx(t);
        let (r0, i0) = region_pitch(o.d.format, &*r);
        let row = if row != 0 { row as usize } else { r0 };
        let img = if img != 0 { img as usize } else { i0.max(row) };
        vk::Gpu::with(|g| g.write_texels(&o, (v as *const view_t).as_ref().copied(), &*r, data, row, img));
    }
    /// # Safety
    /// `t` and `r`, and `v` when non-null, must be readable initialized objects describing a valid
    /// texture copy. `data` must satisfy `Gpu::read_texels`' destination contract for tightly packed
    /// block rows/images: the full `row * rows * max(depth, 1)` extent must fit one live writable
    /// allocation, without arithmetic overflow. No alignment beyond bytes is needed for `data`.
    /// The caller must keep the storage live and free of conflicting accesses through the blocking
    /// call; it must not overlap the runtime's staging source or live shared Rust references.
    fn nvnTextureReadTexels(t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, data: *mut u8) {
        let o = *ctx(t);
        let (row, img) = region_pitch(o.d.format, &*r);
        let (vw, rg, dp) = ((v as *const view_t).as_ref().copied(), *r, data as usize);
        // SAFETY: this unsafe NVN entry's caller provides the writable packed extent and lifetime.
        // region_pitch uses the same format block geometry as read_texels; o/vw/rg are local copies.
        // blocking and Gpu::with invoke the closure synchronously, so dp is not retained.
        // NVN's ABI is void. The backend reports a failed readback and leaves data unchanged.
        let _copied = super::blocking(|| vk::Gpu::with(|g| unsafe { g.read_texels(&o, vw, &rg, dp as *mut u8, row, img) }));
    }
    /// # Safety
    /// The object, destination validity, lifetime and nonoverlap requirements of ReadTexels apply.
    /// Explicit pitches must be nonnegative byte counts. Zero row uses the packed block-row size;
    /// zero img uses max(packed image size, effective row), as below. With those effective pitches,
    /// the caller must provide every row span required by `Gpu::read_texels`, including padding and
    /// all depth slices: `(depth - 1) * img + (rows - 1) * row + row_bytes` for nonempty rows.
    /// All offsets/lengths must fit usize/isize and the destination allocation; these are caller
    /// obligations, not checks made by the guest bridge. Overlapping destination rows copy in order.
    fn nvnTextureReadTexelsStrided(t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, data: *mut u8, row: isize, img: isize) {
        let o = *ctx(t);
        let (r0, i0) = region_pitch(o.d.format, &*r);
        let row = if row != 0 { row as usize } else { r0 };
        let img = if img != 0 { img as usize } else { i0.max(row) };
        let (vw, rg, dp) = ((v as *const view_t).as_ref().copied(), *r, data as usize);
        // SAFETY: this unsafe NVN entry's caller supplies a writable extent for these effective
        // pitches (not just the packed size), with no conflicting aliases/accesses through return.
        // o/vw/rg are local copies, and both wrappers run synchronously without retaining dp.
        // Preserve the void ABI; read_texels itself logs failure instead of fabricating output.
        let _copied = super::blocking(|| vk::Gpu::with(|g| unsafe { g.read_texels(&o, vw, &rg, dp as *mut u8, row, img) }));
    }
    fn nvnTextureFlushTexels(t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion) { let _ = (t, v, r); }
    fn nvnTextureInvalidateTexels(t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion) { let _ = (t, v, r); }

    // ---------- views ----------
    fn nvnTextureViewSetDefaults(v: *mut NVNtextureView) { *vw(v) = view_t::default(); }
    fn nvnTextureViewSetLevels(v: *mut NVNtextureView, base: i32, n: i32) { let o = vw(v); o.bits |= VW_LEVELS; o.base_level = base as u8; o.nlevels = n as u8; }
    fn nvnTextureViewSetLayers(v: *mut NVNtextureView, min: i32, n: i32) { let o = vw(v); o.bits |= VW_LAYERS; o.min_layer = min as u16; o.nlayers = n as u16; }
    fn nvnTextureViewSetFormat(v: *mut NVNtextureView, f: i32) { let o = vw(v); o.bits |= VW_FORMAT; o.format = f as u16; }
    fn nvnTextureViewSetTarget(v: *mut NVNtextureView, t: i32) { let o = vw(v); o.bits |= VW_TARGET; o.target = t as u8; }
    fn nvnTextureViewSetSwizzle(v: *mut NVNtextureView, r: i32, g: i32, b2: i32, a: i32) {
        let o = vw(v);
        o.bits |= VW_SWIZZLE;
        o.swz = [r as u8, g as u8, b2 as u8, a as u8];
    }
    fn nvnTextureViewSetDepthStencilMode(v: *mut NVNtextureView, m: i32) { let o = vw(v); o.bits = (o.bits & 0x3f) | VW_DSMODE | ((m as u8 & 3) << 6); }
    fn nvnTextureViewGetLevels(v: *const NVNtextureView, base: *mut i32, n: *mut i32) -> NVNboolean {
        let o = &*(v as *const view_t);
        *base = o.base_level as i32;
        *n = o.nlevels as i32;
        (o.bits & VW_LEVELS != 0) as u8
    }
    fn nvnTextureViewGetLayers(v: *const NVNtextureView, min: *mut i32, n: *mut i32) -> NVNboolean {
        let o = &*(v as *const view_t);
        *min = o.min_layer as i32;
        *n = o.nlayers as i32;
        (o.bits & VW_LAYERS != 0) as u8
    }
    fn nvnTextureViewGetFormat(v: *const NVNtextureView, f: *mut i32) -> NVNboolean {
        let o = &*(v as *const view_t);
        *f = o.format as i32;
        (o.bits & VW_FORMAT != 0) as u8
    }
    fn nvnTextureViewGetTarget(v: *const NVNtextureView, t: *mut i32) -> NVNboolean {
        let o = &*(v as *const view_t);
        *t = o.target as i32;
        (o.bits & VW_TARGET != 0) as u8
    }
    fn nvnTextureViewCompare(a: *const NVNtextureView, bb: *const NVNtextureView) -> NVNboolean {
        let (x, y) = (&*(a as *const view_t), &*(bb as *const view_t));
        let same = x.bits == y.bits
            && (x.bits & VW_LEVELS == 0 || (x.base_level == y.base_level && x.nlevels == y.nlevels))
            && (x.bits & VW_LAYERS == 0 || (x.min_layer == y.min_layer && x.nlayers == y.nlayers))
            && (x.bits & VW_FORMAT == 0 || x.format == y.format)
            && (x.bits & VW_TARGET == 0 || x.target == y.target)
            && (x.bits & VW_SWIZZLE == 0 || x.swz == y.swz);
        same as u8
    }

    // ---------- samplers ----------
    fn nvnSamplerBuilderSetDefaults(b: *mut NVNsamplerBuilder) {
        let dev = (*(b as *mut samp_builder_t)).dev;
        core::ptr::write_bytes(b as *mut u8, 0, core::mem::size_of::<samp_builder_t>());
        (*(b as *mut samp_builder_t)).dev = dev;
        let o = sb(b);
        // NVN's defaults: linear min and mag without mip filtering, LOD clamp [0, 1000]
        o.minf = min_filter::LINEAR;
        o.magf = 1;
        o.ws = wrap::REPEAT;
        o.wt = wrap::REPEAT;
        o.wr = wrap::REPEAT;
        o.cmp_func = cmp::LESS as u8;
        o.lod_min = 0.0;
        o.lod_max = 1000.0;
        o.aniso = 1.0;
    }
    fn nvnSamplerBuilderSetDevice(b: *mut NVNsamplerBuilder, d: *mut NVNdevice) { (*(b as *mut samp_builder_t)).dev = d; }
    fn nvnSamplerBuilderGetDevice(b: *const NVNsamplerBuilder) -> *mut NVNdevice { (*(b as *const samp_builder_t)).dev }
    fn nvnSamplerBuilderSetMinMagFilter(b: *mut NVNsamplerBuilder, mi: i32, ma: i32) { sb(b).minf = mi as u8; sb(b).magf = ma as u8; }
    fn nvnSamplerBuilderSetWrapMode(b: *mut NVNsamplerBuilder, s: i32, t: i32, r: i32) { let o = sb(b); o.ws = s as u8; o.wt = t as u8; o.wr = r as u8; }
    fn nvnSamplerBuilderSetLodClamp(b: *mut NVNsamplerBuilder, lo: f32, hi: f32) { sb(b).lod_min = lo; sb(b).lod_max = hi; }
    fn nvnSamplerBuilderSetLodBias(b: *mut NVNsamplerBuilder, v: f32) { sb(b).lod_bias = v; }
    fn nvnSamplerBuilderSetCompare(b: *mut NVNsamplerBuilder, m: i32, f: i32) { sb(b).cmp_mode = m as u8; sb(b).cmp_func = f as u8; }
    fn nvnSamplerBuilderSetBorderColor(b: *mut NVNsamplerBuilder, c: *const f32) { sb(b).border = core::ptr::read_unaligned(c as *const [f32; 4]); }
    fn nvnSamplerBuilderSetBorderColori(b: *mut NVNsamplerBuilder, c: *const i32) { for i in 0..4 { sb(b).border[i] = *c.add(i) as f32; } }
    fn nvnSamplerBuilderSetBorderColorui(b: *mut NVNsamplerBuilder, c: *const u32) { for i in 0..4 { sb(b).border[i] = *c.add(i) as f32; } }
    fn nvnSamplerBuilderSetMaxAnisotropy(b: *mut NVNsamplerBuilder, v: f32) { sb(b).aniso = v; }
    fn nvnSamplerBuilderSetReductionFilter(b: *mut NVNsamplerBuilder, v: i32) { sb(b).reduction = v as u8; }
    fn nvnSamplerBuilderSetLodSnap(b: *mut NVNsamplerBuilder, v: f32) { (*(b as *mut samp_builder_t)).lod_snap = v; }
    fn nvnSamplerBuilderGetMinMagFilter(b: *const NVNsamplerBuilder, mi: *mut i32, ma: *mut i32) { *mi = csb(b).minf as i32; *ma = csb(b).magf as i32; }
    fn nvnSamplerBuilderGetWrapMode(b: *const NVNsamplerBuilder, s: *mut i32, t: *mut i32, r: *mut i32) { let o = csb(b); *s = o.ws as i32; *t = o.wt as i32; *r = o.wr as i32; }
    fn nvnSamplerBuilderGetLodClamp(b: *const NVNsamplerBuilder, lo: *mut f32, hi: *mut f32) { *lo = csb(b).lod_min; *hi = csb(b).lod_max; }
    fn nvnSamplerBuilderGetLodBias(b: *const NVNsamplerBuilder) -> f32 { csb(b).lod_bias }
    fn nvnSamplerBuilderGetCompare(b: *const NVNsamplerBuilder, m: *mut i32, f: *mut i32) { *m = csb(b).cmp_mode as i32; *f = csb(b).cmp_func as i32; }
    fn nvnSamplerBuilderGetBorderColor(b: *const NVNsamplerBuilder, c: *mut f32) { core::ptr::write_unaligned(c as *mut [f32; 4], csb(b).border); }
    fn nvnSamplerBuilderGetMaxAnisotropy(b: *const NVNsamplerBuilder) -> f32 { csb(b).aniso }
    fn nvnSamplerBuilderGetReductionFilter(b: *const NVNsamplerBuilder) -> i32 { csb(b).reduction as i32 }
    fn nvnSamplerInitialize(s: *mut NVNsampler, b: *const NVNsamplerBuilder) -> NVNboolean {
        let o = s as *mut samp_t;
        core::ptr::write_bytes(o as *mut u8, 0, core::mem::size_of::<samp_t>());
        (*o).magic = MAGIC_SAMP;
        (*o).p = *csb(b);
        1
    }
    fn nvnSamplerFinalize(s: *mut NVNsampler) { (*(s as *mut samp_t)).magic = 0; }
    fn nvnSamplerSetDebugLabel(s: *mut NVNsampler, l: *const u8) { let _ = (s, l); }
    fn nvnSamplerGetMinMagFilter(s: *const NVNsampler, mi: *mut i32, ma: *mut i32) { let o = &(*(s as *const samp_t)).p; *mi = o.minf as i32; *ma = o.magf as i32; }
    fn nvnSamplerGetWrapMode(s: *const NVNsampler, ws: *mut i32, wt: *mut i32, wr: *mut i32) { let o = &(*(s as *const samp_t)).p; *ws = o.ws as i32; *wt = o.wt as i32; *wr = o.wr as i32; }
    fn nvnSamplerGetLodClamp(s: *const NVNsampler, lo: *mut f32, hi: *mut f32) { let o = &(*(s as *const samp_t)).p; *lo = o.lod_min; *hi = o.lod_max; }
    fn nvnSamplerGetLodBias(s: *const NVNsampler) -> f32 { (*(s as *const samp_t)).p.lod_bias }
    fn nvnSamplerGetMaxAnisotropy(s: *const NVNsampler) -> f32 { (*(s as *const samp_t)).p.aniso }
    fn nvnSamplerCompare(a: *const NVNsampler, bb: *const NVNsampler) -> NVNboolean { ((*(a as *const samp_t)).p == (*(bb as *const samp_t)).p) as u8 }
}

//! Replay of one submission's canonical records into its command buffers.
//!
//! State belongs to each NVN queue (`QState`) and carries over between its submissions, as NVN's command-buffer state
//! does on a queue. Rendering is dynamic rendering over the bound targets, begun at the first draw or clear after a
//! target change and ended by anything that cannot run inside it (copies, dispatches, barriers); a full barrier
//! follows each of those. Window origin: NVN's default lower-left origin is Vulkan's framebuffer orientation once the
//! picture is flipped at present, so viewports are flipped only for an upper-left origin; the winding is inverted
//! whenever they are not (Vulkan measures a triangle's area with the opposite sign).

use super::queue::{Post, Sub};
use super::shader::{InKind, Module, Res};
use super::{layout as L, Gpu};
use crate::nvn::obj::*;
use crate::nvn::types::*;
use crate::tex::format::{self, Kind};
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::sync::Arc;

const DRV_SIZE: usize = 0xA00;

#[derive(Clone, Copy)]
struct Rt {
    tex: Option<tex_t>,
    view: Option<view_t>,
}

pub struct QState {
    pub origin_ll: bool,
    prog: [Option<Arc<Module>>; 6],
    blend: [blend_st; 8],
    color: color_st,
    chmask: u32,
    ms: ms_st,
    poly: poly_st,
    ds: ds_st,
    vattr: [u32; 32],
    nvattr: usize,
    vstream: [vs_st; 16],
    vbuf: [(u64, u64); 16],
    ubo: [[(u64, u64); 16]; 7],
    ssbo: [[(u64, u64); 16]; 7],
    tex: [[u32; 256]; 7],
    samp: [[u32; 128]; 7],
    img: [[u32; 32]; 7],
    tex_pool: usize,
    samp_pool: usize,
    rt_n: usize,
    rt: [Rt; 8],
    rt_depth: Rt,
    vp: [[f32; 4]; 16],
    vp_origin: u32,
    depth_range: [f32; 2],
    scissor: Option<[i32; 4]>,
    stencil: [[u32; 3]; 2],
    poly_off: [f32; 3],
    blend_color: [f32; 4],
    prim_restart: (bool, u32),
    line_width: f32,
    depth_clamp: bool,
    rast_discard: bool,
    patch: u32,
    // per submission
    in_pass: bool,
    pass_extent: vk::Extent2D,
    pass_formats: ([i32; 8], usize, i32, u8),
}

impl QState {
    pub fn new(origin_ll: bool) -> QState {
        let mut blend = [blend_st(0); 8];
        for (i, b) in blend.iter_mut().enumerate() {
            b.set(blend_field::TARGET, i as u32);
            b.set(blend_field::SRC_RGB, blend_func::ONE);
            b.set(blend_field::DST_RGB, blend_func::ZERO);
            b.set(blend_field::SRC_A, blend_func::ONE);
            b.set(blend_field::DST_A, blend_func::ZERO);
            b.set(blend_field::EQ_RGB, blend_eq::ADD);
            b.set(blend_field::EQ_A, blend_eq::ADD);
        }
        let mut ds = ds_st(0);
        ds.set(ds_field::DW, 1);
        ds.set(ds_field::DF, cmp::LESS);
        ds.set(ds_field::SF_F, cmp::ALWAYS);
        ds.set(ds_field::SF_B, cmp::ALWAYS);
        for f in [ds_field::FAIL_F, ds_field::ZFAIL_F, ds_field::ZPASS_F, ds_field::FAIL_B, ds_field::ZFAIL_B, ds_field::ZPASS_B] {
            ds.set(f, stencil_op::KEEP);
        }
        QState {
            origin_ll,
            prog: Default::default(),
            blend,
            color: color_st::new(0, logic_op::COPY, cmp::ALWAYS),
            chmask: 0xffff_ffff,
            ms: ms_st { enable: 1, a2c_dither: 1, ..Default::default() },
            poly: poly_st::new(face::NONE, front_face::CCW, polygon_mode::FILL, 0),
            ds,
            vattr: [0; 32],
            nvattr: 0,
            vstream: [vs_st::default(); 16],
            vbuf: [(0, 0); 16],
            ubo: [[(0, 0); 16]; 7],
            ssbo: [[(0, 0); 16]; 7],
            tex: [[0; 256]; 7],
            samp: [[0; 128]; 7],
            img: [[0; 32]; 7],
            tex_pool: 0,
            samp_pool: 0,
            rt_n: 0,
            rt: [Rt { tex: None, view: None }; 8],
            rt_depth: Rt { tex: None, view: None },
            vp: [[0.0; 4]; 16],
            vp_origin: 0,
            depth_range: [0.0, 1.0],
            scissor: None,
            stencil: [[0xff, 0xff, 0]; 2],
            poly_off: [0.0; 3],
            blend_color: [0.0; 4],
            prim_restart: (false, u32::MAX),
            line_width: 1.0,
            depth_clamp: false,
            rast_discard: false,
            patch: 3,
            in_pass: false,
            pass_extent: vk::Extent2D::default(),
            pass_formats: ([0; 8], 0, 0, 1),
        }
    }
}

fn barrier(g: &Gpu, sub: &Sub) {
    super::queue::full_barrier(g, sub.main);
}

fn end_pass(g: &Gpu, sub: &Sub, qs: &mut QState) {
    if qs.in_pass {
        unsafe { g.dev().cmd_end_rendering(sub.main) };
        qs.in_pass = false;
        barrier(g, sub);
    }
}

pub fn end(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    end_pass(g, sub, qs);
}

/// The image of a texture snapshot for writing (render target, copy destination): made GPU-owned, prepared in the
/// upload command buffer when new. Returns (image, format info, extent, levels, layers, samples).
fn target_image(g: &Gpu, sub: &mut Sub, t: &tex_t) -> Option<(vk::Image, format::TexFormat, vk::Extent3D, vk::SampleCountFlags)> {
    let mut imgs = g.images.lock().unwrap();
    if !g.ensure_image(&mut imgs, t) {
        return None;
    }
    g.tex_gpu_written(&mut imgs, t.serial);
    g.prepare_image(&mut imgs, t.serial, sub);
    let im = &imgs.by_serial[&t.serial];
    Some((im.image, im.tf, im.extent, im.samples))
}
fn source_image(g: &Gpu, sub: &mut Sub, t: &tex_t) -> Option<(vk::Image, format::TexFormat, vk::Extent3D)> {
    let mut imgs = g.images.lock().unwrap();
    if !g.ensure_image(&mut imgs, t) {
        return None;
    }
    g.prepare_image(&mut imgs, t.serial, sub);
    let im = &imgs.by_serial[&t.serial];
    Some((im.image, im.tf, im.extent))
}
fn attach_view(g: &Gpu, sub: &mut Sub, t: &tex_t, v: Option<&view_t>) -> Option<(vk::ImageView, vk::Format, vk::Extent2D, vk::SampleCountFlags, Kind)> {
    let _ = target_image(g, sub, t)?;
    let mut imgs = g.images.lock().unwrap();
    let im = imgs.by_serial.get_mut(&t.serial)?;
    let lvl = v.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
    let e = vk::Extent2D { width: (im.extent.width >> lvl).max(1), height: (im.extent.height >> lvl).max(1) };
    let (fmt, samples, kind) = (im.format, im.samples, im.tf.kind);
    let view = g.image_view(im, v, true)?;
    Some((view, fmt, e, samples, kind))
}

fn begin_pass(g: &Gpu, sub: &mut Sub, qs: &mut QState) -> bool {
    if qs.in_pass {
        return true;
    }
    let mut colors = Vec::new();
    let mut formats = [0i32; 8];
    let mut extent = vk::Extent2D { width: u32::MAX, height: u32::MAX };
    let mut samples = vk::SampleCountFlags::TYPE_1;
    for i in 0..qs.rt_n {
        let r = qs.rt[i];
        let a = r.tex.and_then(|t| attach_view(g, sub, &t, r.view.as_ref()));
        match a {
            Some((view, fmt, e, s, _)) => {
                colors.push(
                    vk::RenderingAttachmentInfo::default()
                        .image_view(view)
                        .image_layout(vk::ImageLayout::GENERAL)
                        .load_op(vk::AttachmentLoadOp::LOAD)
                        .store_op(vk::AttachmentStoreOp::STORE),
                );
                formats[i] = fmt.as_raw();
                extent.width = extent.width.min(e.width);
                extent.height = extent.height.min(e.height);
                samples = s;
            }
            None => colors.push(vk::RenderingAttachmentInfo::default().image_view(vk::ImageView::null())),
        }
    }
    let mut depth = None;
    let mut dfmt = 0;
    if let Some(t) = qs.rt_depth.tex {
        if let Some((view, fmt, e, s, kind)) = attach_view(g, sub, &t, qs.rt_depth.view.as_ref()) {
            depth = Some((view, kind));
            dfmt = fmt.as_raw();
            extent.width = extent.width.min(e.width);
            extent.height = extent.height.min(e.height);
            samples = s;
        }
    }
    if extent.width == u32::MAX {
        crate::log::red_once("a draw or clear with no render target bound");
        return false;
    }
    let da = depth.map(|(v, _)| {
        vk::RenderingAttachmentInfo::default().image_view(v).image_layout(vk::ImageLayout::GENERAL).load_op(vk::AttachmentLoadOp::LOAD).store_op(vk::AttachmentStoreOp::STORE)
    });
    let mut ri = vk::RenderingInfo::default()
        .render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent })
        .layer_count(1)
        .color_attachments(&colors);
    let da_arr;
    if let (Some(d), Some((_, kind))) = (da, depth) {
        da_arr = [d];
        if kind != Kind::Stencil {
            ri = ri.depth_attachment(&da_arr[0]);
        }
        if matches!(kind, Kind::DepthStencil | Kind::Stencil) {
            ri = ri.stencil_attachment(&da_arr[0]);
        }
    }
    unsafe { g.dev().cmd_begin_rendering(sub.main, &ri) };
    qs.in_pass = true;
    qs.pass_extent = extent;
    qs.pass_formats = (formats, qs.rt_n, dfmt, samples.as_raw() as u8);
    true
}

fn blend_factor(f: u32) -> vk::BlendFactor {
    use vk::BlendFactor as B;
    match f {
        1 => B::ZERO,
        2 => B::ONE,
        3 => B::SRC_COLOR,
        4 => B::ONE_MINUS_SRC_COLOR,
        5 => B::SRC_ALPHA,
        6 => B::ONE_MINUS_SRC_ALPHA,
        7 => B::DST_ALPHA,
        8 => B::ONE_MINUS_DST_ALPHA,
        9 => B::DST_COLOR,
        10 => B::ONE_MINUS_DST_COLOR,
        11 => B::SRC_ALPHA_SATURATE,
        16 => B::SRC1_COLOR,
        17 => B::ONE_MINUS_SRC1_COLOR,
        18 => B::SRC1_ALPHA,
        19 => B::ONE_MINUS_SRC1_ALPHA,
        97 => B::CONSTANT_COLOR,
        98 => B::ONE_MINUS_CONSTANT_COLOR,
        99 => B::CONSTANT_ALPHA,
        100 => B::ONE_MINUS_CONSTANT_ALPHA,
        _ => B::ONE,
    }
}
fn blend_op(e: u32) -> vk::BlendOp {
    match e {
        2 => vk::BlendOp::SUBTRACT,
        3 => vk::BlendOp::REVERSE_SUBTRACT,
        4 => vk::BlendOp::MIN,
        5 => vk::BlendOp::MAX,
        _ => vk::BlendOp::ADD,
    }
}
fn stencil_op_vk(o: u32) -> vk::StencilOp {
    use vk::StencilOp as S;
    match o {
        2 => S::ZERO,
        3 => S::REPLACE,
        4 => S::INCREMENT_AND_CLAMP,
        5 => S::DECREMENT_AND_CLAMP,
        6 => S::INVERT,
        7 => S::INCREMENT_AND_WRAP,
        8 => S::DECREMENT_AND_WRAP,
        _ => S::KEEP,
    }
}
fn topology(p: i32) -> vk::PrimitiveTopology {
    use vk::PrimitiveTopology as T;
    match p {
        prim::POINTS => T::POINT_LIST,
        prim::LINES => T::LINE_LIST,
        prim::LINE_LOOP | prim::LINE_STRIP => T::LINE_STRIP,
        prim::TRIANGLES | prim::QUADS => T::TRIANGLE_LIST,
        prim::TRIANGLE_STRIP | prim::QUAD_STRIP => T::TRIANGLE_STRIP,
        prim::TRIANGLE_FAN | prim::POLYGON => T::TRIANGLE_FAN,
        prim::LINES_ADJACENCY => T::LINE_LIST_WITH_ADJACENCY,
        prim::LINE_STRIP_ADJACENCY => T::LINE_STRIP_WITH_ADJACENCY,
        prim::TRIANGLES_ADJACENCY => T::TRIANGLE_LIST_WITH_ADJACENCY,
        prim::TRIANGLE_STRIP_ADJACENCY => T::TRIANGLE_STRIP_WITH_ADJACENCY,
        _ => T::PATCH_LIST,
    }
}

/// A zero-filled buffer that stands in for unbound uniform, storage and vertex bindings.
fn zero_buffer(g: &Gpu, sub: &mut Sub) -> vk::Buffer {
    static ZERO: std::sync::OnceLock<super::Buf> = std::sync::OnceLock::new();
    let mut fresh = false;
    let b = ZERO.get_or_init(|| {
        fresh = true;
        g.buffer(
            65536,
            vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            "zero",
        )
        .expect("zero buffer")
    });
    if fresh {
        unsafe { g.dev().cmd_fill_buffer(sub.pre, b.buf, 0, vk::WHOLE_SIZE, 0) };
        super::queue::full_barrier(g, sub.pre);
    }
    b.buf
}

/// The driver's constant segment c0 of a stage: storage buffer table, compute dispatch size and uniform table, base
/// instance.
fn c0_bytes(qs: &QState, st: usize, dispatch: [u32; 3], base_instance: u32) -> Vec<u8> {
    let mut v = vec![0u8; DRV_SIZE];
    let base = match st {
        0 => 0x110,
        5 => 0x310,
        2 => 0x410,
        1 => 0x510,
        _ => 0,
    };
    if base != 0 {
        for r in 0..16 {
            let (a, n) = qs.ssbo[st][r];
            let o = base + 0x10 * r;
            v[o..o + 8].copy_from_slice(&a.to_le_bytes());
            v[o + 8..o + 12].copy_from_slice(&(n as u32).to_le_bytes());
        }
    }
    if st == 5 {
        for (i, d) in dispatch.iter().enumerate() {
            v[0x200 + 4 * i..0x204 + 4 * i].copy_from_slice(&d.to_le_bytes());
        }
        for r in 0..16 {
            let (a, n) = qs.ubo[5][r];
            let o = 0x210 + 0x10 * r;
            if o + 12 <= 0x310 {
                v[o..o + 8].copy_from_slice(&a.to_le_bytes());
                v[o + 8..o + 12].copy_from_slice(&(n as u32).to_le_bytes());
            }
        }
    } else {
        v[0x644..0x648].copy_from_slice(&base_instance.to_le_bytes());
    }
    v
}

/// The handle table c2 of a stage: one 32-bit handle word every 8 bytes per slot.
fn c2_bytes(qs: &QState, st: usize) -> Vec<u8> {
    let mut v = vec![0u8; DRV_SIZE];
    let mut put = |o: usize, h: u32| v[o..o + 4].copy_from_slice(&h.to_le_bytes());
    for r in 0..41 {
        put(0x20 + 8 * r, qs.tex[st][r]);
    }
    for r in 0..8 {
        put(0x120 + 8 * r, qs.img[st][r]);
    }
    for r in 0..128 {
        put(0x168 + 8 * r, qs.tex[st][128 + r]);
        put(0x568 + 8 * r, qs.samp[st][r]);
    }
    v
}

/// The words of constant bank `slot` of a stage as the shader reads them: c0 / c2 as the host builds them, c1 the
/// shader's constants, c3 + r the game's uniform buffer (read from guest memory).
fn bank_word(qs: &QState, st: usize, m: &Module, c0: &[u8], c2: &[u8], slot: i32, word: u32) -> u32 {
    let at = 4 * word as usize;
    let rd = |b: &[u8]| -> u32 { b.get(at..at + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0) };
    match slot {
        0 => rd(c0),
        -1 | 2 => rd(c2),
        1 => m.c1.as_deref().map(rd).unwrap_or(0),
        s if (3..19).contains(&s) => {
            let (a, n) = qs.ubo[st][(s - 3) as usize];
            if a == 0 || at as u64 + 4 > n {
                0
            } else {
                unsafe { core::ptr::read_unaligned((a + at as u64) as *const u32) }
            }
        }
        _ => 0,
    }
}

fn bank_u64(qs: &QState, st: usize, m: &Module, c0: &[u8], c2: &[u8], slot: i32, byte: u32) -> (u64, u64) {
    let lo = bank_word(qs, st, m, c0, c2, slot, byte / 4) as u64;
    let hi = bank_word(qs, st, m, c0, c2, slot, byte / 4 + 1) as u64;
    let n = bank_word(qs, st, m, c0, c2, slot, byte / 4 + 2) as u64;
    (lo | hi << 32, n)
}

/// The NVN target a shader's sampler type asks for (dimension | array 0x100 | multisample 0x400).
fn target_of(ty: u32) -> i32 {
    let arr = ty & 0x100 != 0;
    match ty & 0xff {
        1 => if arr { target::T1D_ARRAY } else { target::T1D },
        4 => target::T3D,
        5 => if arr { target::CUBE_ARRAY } else { target::CUBE },
        _ if ty & 0x400 != 0 => if arr { target::T2D_MS_ARRAY } else { target::T2D_MS },
        _ => if arr { target::T2D_ARRAY } else { target::T2D },
    }
}

/// The view of a registered texture ID with the type the shader declares; a stand-in when there is none.
fn tex_view(g: &Gpu, sub: &mut Sub, id: i32, ty: u32, storage: bool) -> vk::ImageView {
    let want = target_of(ty);
    if let Some((serial, view)) = g.tables.tex_at(id) {
        let mut imgs = g.images.lock().unwrap();
        if let Some(im) = imgs.by_serial.get_mut(&serial) {
            let mut v = view.unwrap_or_default();
            let base_target = if v.bits & VW_TARGET != 0 { v.target as i32 } else { im.tex.d.target };
            let compatible = match (base_target, want) {
                (a, b) if a == b => true,
                (target::T2D | target::RECTANGLE, target::T2D) => true,
                (target::T2D | target::T2D_ARRAY | target::CUBE | target::CUBE_ARRAY | target::RECTANGLE, target::T2D_ARRAY | target::T2D) => true,
                (target::CUBE_ARRAY, target::CUBE) => true,
                _ => false,
            };
            if compatible && (!storage || im.usage.contains(vk::ImageUsageFlags::STORAGE)) {
                if base_target != want {
                    v.bits |= VW_TARGET;
                    v.target = want as u8;
                }
                if let Some(view) = g.image_view(im, Some(&v), false) {
                    return view;
                }
            } else {
                crate::log::red_once(&format!("texture ID {id}: target {base_target} bound where the shader samples target {want}"));
            }
        }
    }
    dummy_view(g, sub, want, storage)
}

/// 1x1 zero images standing in for unbound textures, one per view type.
fn dummy_view(g: &Gpu, sub: &mut Sub, target: i32, storage: bool) -> vk::ImageView {
    static DUMMY: std::sync::Mutex<Option<(vk::Image, vk::Image, Vec<(i32, bool, vk::ImageView)>)>> = std::sync::Mutex::new(None);
    let mut d = DUMMY.lock().unwrap();
    if d.is_none() {
        let mk = |ty: vk::ImageType, layers: u32, cube: bool| {
            let ci = vk::ImageCreateInfo::default()
                .flags(if cube { vk::ImageCreateFlags::CUBE_COMPATIBLE } else { vk::ImageCreateFlags::empty() })
                .image_type(ty)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D { width: 1, height: 1, depth: 1 })
                .mip_levels(1)
                .array_layers(layers)
                .samples(vk::SampleCountFlags::TYPE_1)
                .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST);
            let i = unsafe { g.dev().create_image(&ci, None) }.unwrap();
            let req = unsafe { g.dev().get_image_memory_requirements(i) };
            let a = g
                .alloc
                .lock()
                .unwrap()
                .allocate(&gpu_allocator::vulkan::AllocationCreateDesc { name: "dummy", requirements: req, location: MemoryLocation::GpuOnly, linear: false, allocation_scheme: gpu_allocator::vulkan::AllocationScheme::GpuAllocatorManaged })
                .unwrap();
            unsafe { g.dev().bind_image_memory(i, a.memory(), a.offset()) }.unwrap();
            std::mem::forget(a);
            i
        };
        let i2 = mk(vk::ImageType::TYPE_2D, 6, true);
        let i3 = mk(vk::ImageType::TYPE_3D, 1, false);
        for (im, layers) in [(i2, 6), (i3, 1)] {
            let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: layers };
            let b = [vk::ImageMemoryBarrier2::default()
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(im)
                .subresource_range(range)];
            unsafe {
                g.dev().cmd_pipeline_barrier2(sub.pre, &vk::DependencyInfo::default().image_memory_barriers(&b));
                g.dev().cmd_clear_color_image(sub.pre, im, vk::ImageLayout::GENERAL, &vk::ClearColorValue { float32: [0.0; 4] }, &[range]);
            }
        }
        super::queue::full_barrier(g, sub.pre);
        *d = Some((i2, i3, Vec::new()));
    }
    let (i2, i3, views) = d.as_mut().unwrap();
    if let Some(v) = views.iter().find(|v| v.0 == target && v.1 == storage) {
        return v.2;
    }
    let (img, vt, layers) = match target {
        target::T3D => (*i3, vk::ImageViewType::TYPE_3D, 1),
        target::CUBE => (*i2, vk::ImageViewType::CUBE, 6),
        target::CUBE_ARRAY => (*i2, vk::ImageViewType::CUBE_ARRAY, 6),
        target::T2D_ARRAY | target::T2D_MS_ARRAY => (*i2, vk::ImageViewType::TYPE_2D_ARRAY, 1),
        _ => (*i2, vk::ImageViewType::TYPE_2D, 1),
    };
    let mut usage = vk::ImageViewUsageCreateInfo::default().usage(if storage { vk::ImageUsageFlags::STORAGE } else { vk::ImageUsageFlags::SAMPLED });
    let ci = vk::ImageViewCreateInfo::default()
        .image(img)
        .view_type(vt)
        .format(vk::Format::R8G8B8A8_UNORM)
        .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: layers })
        .push_next(&mut usage);
    let v = unsafe { g.dev().create_image_view(&ci, None) }.unwrap();
    views.push((target, storage, v));
    v
}

fn dummy_sampler(g: &Gpu) -> vk::Sampler {
    let p = samp_par { minf: 1, magf: 1, ws: 1, wt: 1, wr: 1, cmp_func: 2, lod_max: 1000.0, aniso: 1.0, ..Default::default() };
    g.tables.sampler(&g.d, &p)
}

/// Ryujinx's support buffer as lane C's shaders read it: alpha test off, no BGRA targets, the viewport's inverse and
/// size, render scale 1.
fn support_bytes(w: f32, h: f32) -> Vec<u8> {
    let mut v = vec![0u8; 1392];
    let put = |v: &mut Vec<u8>, o: usize, f: f32| v[o..o + 4].copy_from_slice(&f.to_le_bytes());
    put(&mut v, 144, 2.0 / w.max(1.0));
    put(&mut v, 148, 2.0 / h.max(1.0));
    put(&mut v, 152, 1.0);
    put(&mut v, 160, w);
    put(&mut v, 164, h);
    put(&mut v, 168, 1.0);
    for i in 0..73 {
        for k in 0..4 {
            put(&mut v, 192 + 16 * i + 4 * k, 1.0);
        }
    }
    v
}

/// The descriptor set of one stage of the draw or dispatch.
fn stage_set(g: &Gpu, sub: &mut Sub, qs: &QState, m: &Module, dispatch: [u32; 3], base_instance: u32) -> vk::DescriptorSet {
    let st = m.stage as usize;
    let set = sub.alloc_set(g, g.layout.stage_sets[st]);
    let zero = zero_buffer(g, sub);
    let max_ubo = (g.d.props.limits.max_uniform_buffer_range as u64).min(65536);
    let c0 = c0_bytes(qs, st, dispatch, base_instance);
    let c2 = c2_bytes(qs, st);
    let (tb, sb) = pool_bases(qs);
    let mut bufs: Vec<(u32, vk::DescriptorType, vk::DescriptorBufferInfo)> = Vec::new();
    let mut imgs: Vec<(u32, vk::DescriptorType, vk::DescriptorImageInfo)> = Vec::new();
    for (b, r) in &m.res {
        let b = *b;
        match r {
            Res::Support => {
                let (w, h) = (qs.pass_extent.width.max(1) as f32, qs.pass_extent.height.max(1) as f32);
                let (buf, off) = sub.stage_bytes(g, &support_bytes(w, h));
                bufs.push((b, vk::DescriptorType::UNIFORM_BUFFER, vk::DescriptorBufferInfo { buffer: buf, offset: off, range: 1392 }));
            }
            Res::Cbuf { bank } => {
                let info = match *bank {
                    0 => {
                        let (buf, off) = sub.stage_bytes(g, &c0);
                        vk::DescriptorBufferInfo { buffer: buf, offset: off, range: DRV_SIZE as u64 }
                    }
                    1 => match &m.c1 {
                        Some(c1) if !c1.is_empty() => {
                            let (buf, off) = sub.stage_bytes(g, c1);
                            vk::DescriptorBufferInfo { buffer: buf, offset: off, range: (c1.len() as u64).min(max_ubo) }
                        }
                        _ => vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: max_ubo },
                    },
                    2 => {
                        let (buf, off) = sub.stage_bytes(g, &c2);
                        vk::DescriptorBufferInfo { buffer: buf, offset: off, range: DRV_SIZE as u64 }
                    }
                    k @ 3..=18 => {
                        let (a, n) = qs.ubo[st][(k - 3) as usize];
                        match if a != 0 && n != 0 { super::mem::resolve(g, sub, a, n.min(max_ubo)) } else { None } {
                            Some((buf, off, _)) => {
                                if off % g.d.caps.uniform_align != 0 {
                                    crate::log::red_once(&format!("uniform buffer at {a:#x}: offset not aligned to {}", g.d.caps.uniform_align));
                                }
                                vk::DescriptorBufferInfo { buffer: buf, offset: off, range: n.min(max_ubo) }
                            }
                            None => vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: max_ubo },
                        }
                    }
                    _ => vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: max_ubo },
                };
                bufs.push((b, vk::DescriptorType::UNIFORM_BUFFER, info));
            }
            Res::Sbuf { cb_slot, cb_off, written } => {
                let (a, n) = bank_u64(qs, st, m, &c0, &c2, *cb_slot, *cb_off);
                let info = match if a != 0 && n != 0 { super::mem::resolve(g, sub, a, n) } else { None } {
                    Some((buf, off, _)) => {
                        if *written {
                            super::mem::note_gpu_write(g, sub, a, n);
                        }
                        if off % g.d.caps.storage_align != 0 {
                            crate::log::red_once(&format!("storage buffer at {a:#x}: offset not aligned to {}", g.d.caps.storage_align));
                        }
                        vk::DescriptorBufferInfo { buffer: buf, offset: off, range: n }
                    }
                    None => vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: 65536 },
                };
                bufs.push((b, vk::DescriptorType::STORAGE_BUFFER, info));
            }
            Res::Tex { handle, cbuf_slot, ty } | Res::Img { handle, cbuf_slot, ty } => {
                let is_img = matches!(r, Res::Img { .. });
                if *ty == 2 {
                    crate::log::red_once("buffer textures are not bound yet");
                    continue;
                }
                let word = bank_word(qs, st, m, &c0, &c2, *cbuf_slot, handle & 0x3fff);
                let kind = handle >> 28;
                let tid = (word & 0xfffff) as i32 + tb;
                let sid = match kind {
                    0 => (word >> 20) as i32,
                    1 => (bank_word(qs, st, m, &c0, &c2, *cbuf_slot, handle >> 14 & 0x3fff) >> 20) as i32,
                    2 => (handle >> 14 & 0x3fff) as i32,
                    _ => {
                        crate::log::red_once("constant sampler handles are not bound yet");
                        0
                    }
                } + sb;
                let view = tex_view(g, sub, tid, *ty, is_img);
                if is_img {
                    imgs.push((b, vk::DescriptorType::STORAGE_IMAGE, vk::DescriptorImageInfo { sampler: vk::Sampler::null(), image_view: view, image_layout: vk::ImageLayout::GENERAL }));
                } else {
                    let s = g.tables.samp_at(sid).unwrap_or_else(|| dummy_sampler(g));
                    imgs.push((b, vk::DescriptorType::COMBINED_IMAGE_SAMPLER, vk::DescriptorImageInfo { sampler: s, image_view: view, image_layout: vk::ImageLayout::GENERAL }));
                }
            }
            Res::Pool => {}
        }
    }
    let mut writes: Vec<vk::WriteDescriptorSet> = bufs
        .iter()
        .map(|(b, ty, i)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*b).descriptor_type(*ty).buffer_info(std::slice::from_ref(i)))
        .collect();
    writes.extend(imgs.iter().map(|(b, ty, i)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*b).descriptor_type(*ty).image_info(std::slice::from_ref(i))));
    unsafe { g.dev().update_descriptor_sets(&writes, &[]) };
    set
}

fn pool_bases(qs: &QState) -> (i32, i32) {
    let tb = if qs.tex_pool != 0 { crate::nvn::pool::dpool_base_of(qs.tex_pool as *const dpool_t, true) } else { 0 };
    let sb = if qs.samp_pool != 0 { crate::nvn::pool::dpool_base_of(qs.samp_pool as *const dpool_t, false) } else { 0 };
    (tb, sb)
}

fn push_constants(g: &Gpu, sub: &Sub, qs: &QState, flipped: bool) {
    let (tb, sb) = pool_bases(qs);
    let pc: [u32; 4] = [tb as u32, sb as u32, flipped as u32, 0];
    let bytes = unsafe { core::slice::from_raw_parts(pc.as_ptr() as *const u8, 16) };
    unsafe { g.dev().cmd_push_constants(sub.main, g.layout.pipeline, L::PUSH_STAGES, 0, bytes) };
}

fn bind_sets(g: &Gpu, sub: &mut Sub, qs: &QState, mods: &[Arc<Module>], bp: vk::PipelineBindPoint, dispatch: [u32; 3], base_instance: u32) {
    let mut pool = false;
    for m in mods {
        let set = stage_set(g, sub, qs, m, dispatch, base_instance);
        unsafe { g.dev().cmd_bind_descriptor_sets(sub.main, bp, g.layout.pipeline, m.stage, &[set], &[]) };
        pool |= m.uses_pool;
    }
    if pool {
        unsafe { g.dev().cmd_bind_descriptor_sets(sub.main, bp, g.layout.pipeline, L::POOL_SET, &[g.tables.set], &[]) };
    }
}

/// Everything a draw needs bound; false when it cannot be drawn (it is skipped).
fn draw_setup(g: &Gpu, sub: &mut Sub, qs: &mut QState, prim_ty: i32, base_instance: u32) -> bool {
    let Some(vs) = qs.prog[0].clone() else {
        crate::log::red_once("draw with no vertex shader bound");
        return false;
    };
    if !begin_pass(g, sub, qs) {
        return false;
    }
    let mods: Vec<Arc<Module>> = [0usize, 1, 2, 3, 4].iter().filter_map(|&s| qs.prog[s].clone()).collect();
    let mut key = super::pipe::GfxKey::default();
    for m in &mods {
        key.shaders[m.stage as usize] = m.key;
    }
    let (formats, n, dfmt, samples) = qs.pass_formats;
    key.colors = formats;
    key.ncolor = n as u8;
    key.depth = dfmt;
    key.samples = samples;
    let Some(pipe) = g.gfx_pipeline(&key, &mods) else { return false };
    let dev = g.dev();
    let cb = sub.main;
    unsafe { dev.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipe) };
    bind_sets(g, sub, qs, &mods, vk::PipelineBindPoint::GRAPHICS, [0; 3], base_instance);

    // viewport and origin
    let o = if qs.vp_origin != 0 { qs.vp_origin == 2 } else { !qs.origin_ll };
    let upper_left = o;
    push_constants(g, sub, qs, upper_left);
    let [x, y, w, h] = qs.vp[0];
    let (w, h) = if w == 0.0 || h == 0.0 { (qs.pass_extent.width as f32, qs.pass_extent.height as f32) } else { (w, h) };
    let vp = if upper_left {
        vk::Viewport { x, y: y + h, width: w, height: -h, min_depth: qs.depth_range[0].clamp(0.0, 1.0), max_depth: qs.depth_range[1].clamp(0.0, 1.0) }
    } else {
        vk::Viewport { x, y, width: w, height: h, min_depth: qs.depth_range[0].clamp(0.0, 1.0), max_depth: qs.depth_range[1].clamp(0.0, 1.0) }
    };
    let sc = match qs.scissor {
        Some([sx, sy, sw, sh]) => vk::Rect2D { offset: vk::Offset2D { x: sx.max(0), y: sy.max(0) }, extent: vk::Extent2D { width: sw.max(0) as u32, height: sh.max(0) as u32 } },
        None => vk::Rect2D { offset: vk::Offset2D::default(), extent: qs.pass_extent },
    };
    let ds = qs.ds;
    let poly = qs.poly;
    unsafe {
        dev.cmd_set_viewport_with_count(cb, &[vp]);
        dev.cmd_set_scissor_with_count(cb, &[sc]);
        if qs.line_width != 1.0 {
            crate::log::red_once("line widths other than 1 are drawn 1 pixel wide");
        }
        dev.cmd_set_line_width(cb, 1.0);
        let off_on = poly.offset_en() & (1 << poly.mode().min(2)) != 0;
        dev.cmd_set_depth_bias_enable(cb, off_on);
        dev.cmd_set_depth_bias(cb, qs.poly_off[1], qs.poly_off[2], qs.poly_off[0]);
        dev.cmd_set_blend_constants(cb, &qs.blend_color);
        dev.cmd_set_depth_bounds(cb, 0.0, 1.0);
        dev.cmd_set_depth_bounds_test_enable(cb, false);
        for (i, f) in [vk::StencilFaceFlags::FRONT, vk::StencilFaceFlags::BACK].iter().enumerate() {
            dev.cmd_set_stencil_compare_mask(cb, *f, qs.stencil[i][0]);
            dev.cmd_set_stencil_write_mask(cb, *f, qs.stencil[i][1]);
            dev.cmd_set_stencil_reference(cb, *f, qs.stencil[i][2]);
        }
        dev.cmd_set_cull_mode(cb, vk::CullModeFlags::from_raw(poly.cull()));
        // NVN CW = 0 / CCW = 1; Vulkan COUNTER_CLOCKWISE = 0 / CLOCKWISE = 1. Unflipped, the winding inverts.
        let ccw = poly.front() == front_face::CCW;
        let vk_ccw = if upper_left { ccw } else { !ccw };
        dev.cmd_set_front_face(cb, if vk_ccw { vk::FrontFace::COUNTER_CLOCKWISE } else { vk::FrontFace::CLOCKWISE });
        dev.cmd_set_primitive_topology(cb, topology(prim_ty));
        dev.cmd_set_depth_test_enable(cb, ds.get(ds_field::DT) != 0);
        dev.cmd_set_depth_write_enable(cb, ds.get(ds_field::DW) != 0);
        dev.cmd_set_depth_compare_op(cb, super::tables::compare_op(ds.get(ds_field::DF)));
        dev.cmd_set_stencil_test_enable(cb, ds.get(ds_field::ST) != 0);
        dev.cmd_set_stencil_op(
            cb,
            vk::StencilFaceFlags::FRONT,
            stencil_op_vk(ds.get(ds_field::FAIL_F)),
            stencil_op_vk(ds.get(ds_field::ZPASS_F)),
            stencil_op_vk(ds.get(ds_field::ZFAIL_F)),
            super::tables::compare_op(ds.get(ds_field::SF_F)),
        );
        dev.cmd_set_stencil_op(
            cb,
            vk::StencilFaceFlags::BACK,
            stencil_op_vk(ds.get(ds_field::FAIL_B)),
            stencil_op_vk(ds.get(ds_field::ZPASS_B)),
            stencil_op_vk(ds.get(ds_field::ZFAIL_B)),
            super::tables::compare_op(ds.get(ds_field::SF_B)),
        );
        dev.cmd_set_rasterizer_discard_enable(cb, qs.rast_discard);
        dev.cmd_set_primitive_restart_enable(cb, qs.prim_restart.0 && !matches!(prim_ty, prim::POINTS | prim::LINES | prim::TRIANGLES | prim::QUADS | prim::PATCHES));
        let caps = g.d.caps;
        if caps.logic_op_dyn {
            g.d.eds2.cmd_set_logic_op(cb, vk::LogicOp::from_raw(qs.color.logic_op() as i32));
        }
        if caps.patch_dyn && mods.iter().any(|m| m.stage == 3 || m.stage == 4) {
            g.d.eds2.cmd_set_patch_control_points(cb, qs.patch.max(1));
        }
        let e3 = &g.d.eds3;
        if caps.eds3_poly {
            e3.cmd_set_polygon_mode(cb, match poly.mode() { 0 => vk::PolygonMode::POINT, 1 => vk::PolygonMode::LINE, _ => vk::PolygonMode::FILL });
        }
        let sflag = vk::SampleCountFlags::from_raw(samples.max(1) as u32);
        if caps.eds3_samples {
            e3.cmd_set_rasterization_samples(cb, sflag);
        }
        if caps.eds3_sample_mask {
            e3.cmd_set_sample_mask(cb, sflag, &[u32::MAX]);
        }
        if caps.eds3_a2c {
            e3.cmd_set_alpha_to_coverage_enable(cb, qs.ms.a2c != 0);
        }
        if caps.eds3_logic_en {
            e3.cmd_set_logic_op_enable(cb, qs.color.logic_op() != logic_op::COPY);
        }
        if caps.eds3_clamp {
            e3.cmd_set_depth_clamp_enable(cb, qs.depth_clamp);
        }
        if caps.eds3_clip_neg {
            e3.cmd_set_depth_clip_negative_one_to_one(cb, !crate::nvn::device::DEPTH_ZERO.load(std::sync::atomic::Ordering::Relaxed));
        }
        if n > 0 {
            let en: Vec<vk::Bool32> = (0..n).map(|i| (qs.color.blend_en() >> i & 1) as vk::Bool32).collect();
            let eq: Vec<vk::ColorBlendEquationEXT> = (0..n)
                .map(|i| {
                    let b = qs.blend[i];
                    vk::ColorBlendEquationEXT {
                        src_color_blend_factor: blend_factor(b.get(blend_field::SRC_RGB)),
                        dst_color_blend_factor: blend_factor(b.get(blend_field::DST_RGB)),
                        color_blend_op: blend_op(b.get(blend_field::EQ_RGB)),
                        src_alpha_blend_factor: blend_factor(b.get(blend_field::SRC_A)),
                        dst_alpha_blend_factor: blend_factor(b.get(blend_field::DST_A)),
                        alpha_blend_op: blend_op(b.get(blend_field::EQ_A)),
                    }
                })
                .collect();
            let wm: Vec<vk::ColorComponentFlags> = (0..n).map(|i| vk::ColorComponentFlags::from_raw(qs.chmask >> (4 * i) & 0xf)).collect();
            e3.cmd_set_color_blend_enable(cb, 0, &en);
            e3.cmd_set_color_blend_equation(cb, 0, &eq);
            e3.cmd_set_color_write_mask(cb, 0, &wm);
        }
    }
    if qs.color.alpha_func() != cmp::ALWAYS && qs.color.alpha_func() != 0 {
        crate::log::red_once("alpha test is not implemented yet (it belongs in the translated fragment shader)");
    }
    vertex_input(g, sub, qs, &vs);
    true
}

fn vertex_input(g: &Gpu, sub: &mut Sub, qs: &QState, vs: &Module) {
    let mut binds: Vec<vk::VertexInputBindingDescription2EXT> = Vec::new();
    let mut attrs: Vec<vk::VertexInputAttributeDescription2EXT> = Vec::new();
    let mut streams_used = 0u32;
    let mut need_zero = false;
    for &(loc, kind) in &vs.inputs {
        let a = va_st(if (loc as usize) < qs.nvattr { qs.vattr[loc as usize] } else { 0 });
        if a.fmt() != 0 {
            let s = a.stream().min(15);
            streams_used |= 1 << s;
            attrs.push(vk::VertexInputAttributeDescription2EXT::default().location(loc).binding(s).format(format::vertex_format(a.fmt() as i32)).offset(a.offset()));
        } else {
            need_zero = true;
            let f = match kind {
                InKind::Float => vk::Format::R32G32B32A32_SFLOAT,
                InKind::Sint => vk::Format::R32G32B32A32_SINT,
                InKind::Uint => vk::Format::R32G32B32A32_UINT,
            };
            attrs.push(vk::VertexInputAttributeDescription2EXT::default().location(loc).binding(31).format(f).offset(0));
        }
    }
    let mut bufs: Vec<(u32, vk::Buffer, u64)> = Vec::new();
    for s in 0..16u32 {
        if streams_used & (1 << s) == 0 {
            continue;
        }
        let st = qs.vstream[s as usize];
        let (rate, div) = if st.divisor == 0 { (vk::VertexInputRate::VERTEX, 1) } else { (vk::VertexInputRate::INSTANCE, st.divisor) };
        let div = if div > 1 && !g.d.caps.divisor {
            crate::log::red_once("vertex divisor > 1 without VK_EXT_vertex_attribute_divisor");
            1
        } else {
            div
        };
        binds.push(vk::VertexInputBindingDescription2EXT::default().binding(s).stride(st.stride).input_rate(rate).divisor(div));
        let (a, n) = qs.vbuf[s as usize];
        match if a != 0 { super::mem::resolve(g, sub, a, n.max(1)) } else { None } {
            Some((b, o, _)) => bufs.push((s, b, o)),
            None => {
                let z = zero_buffer(g, sub);
                bufs.push((s, z, 0));
            }
        }
    }
    if need_zero {
        binds.push(vk::VertexInputBindingDescription2EXT::default().binding(31).stride(0).input_rate(vk::VertexInputRate::VERTEX).divisor(1));
        let z = zero_buffer(g, sub);
        bufs.push((31, z, 0));
    }
    unsafe {
        g.d.vin.cmd_set_vertex_input(sub.main, &binds, &attrs);
        for (s, b, o) in bufs {
            g.dev().cmd_bind_vertex_buffers(sub.main, s, &[b], &[o]);
        }
    }
}

/// Index data of a quads draw as triangles (u32), from a vertex range or from the game's index buffer.
fn quads_indices(n: u32, src: Option<(&[u8], i32)>) -> Vec<u32> {
    let get = |i: u32| -> u32 {
        match src {
            None => i,
            Some((b, ty)) => match ty {
                index_type::U8 => b.get(i as usize).copied().unwrap_or(0) as u32,
                index_type::U16 => u16::from_le_bytes([b[2 * i as usize], b[2 * i as usize + 1]]) as u32,
                _ => u32::from_le_bytes(b[4 * i as usize..4 * i as usize + 4].try_into().unwrap()),
            },
        }
    };
    let mut v = Vec::with_capacity((n / 4 * 6) as usize);
    for q in 0..n / 4 {
        let b = q * 4;
        v.extend_from_slice(&[get(b), get(b + 1), get(b + 2), get(b), get(b + 2), get(b + 3)]);
    }
    v
}

fn draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_draw) {
    let inst = if d.instances <= 0 { 1 } else { d.instances as u32 };
    if !draw_setup(g, sub, qs, d.prim, d.base_instance as u32) {
        return;
    }
    let dev = g.dev();
    if d.ind != 0 {
        if let Some((b, o, _)) = super::mem::resolve(g, sub, d.ind, 16) {
            unsafe { dev.cmd_draw_indirect(sub.main, b, o, 1, 16) };
        }
        return;
    }
    if d.prim == prim::QUADS {
        let idx = quads_indices(d.count as u32, None);
        let bytes = unsafe { core::slice::from_raw_parts(idx.as_ptr() as *const u8, idx.len() * 4) };
        let (b, o) = sub.stage_bytes(g, bytes);
        unsafe {
            dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT32);
            dev.cmd_draw_indexed(sub.main, idx.len() as u32, inst, 0, d.first, d.base_instance as u32);
        }
        return;
    }
    if d.prim == prim::LINE_LOOP {
        crate::log::red_once("LINE_LOOP drawn as a line strip (the closing segment is missing)");
    }
    unsafe { dev.cmd_draw(sub.main, d.count as u32, inst, d.first as u32, d.base_instance as u32) };
}

fn draw_idx(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_draw_idx) {
    let inst = if d.instances <= 0 { 1 } else { d.instances as u32 };
    let isz = match d.itype {
        index_type::U8 => 1u64,
        index_type::U16 => 2,
        _ => 4,
    };
    if !draw_setup(g, sub, qs, d.prim, d.base_instance as u32) {
        return;
    }
    let dev = g.dev();
    if d.prim == prim::QUADS && d.ind == 0 {
        let src = unsafe { core::slice::from_raw_parts(d.ibuf as *const u8, (d.count as u64 * isz) as usize) };
        let idx = quads_indices(d.count as u32, Some((src, d.itype)));
        let bytes = unsafe { core::slice::from_raw_parts(idx.as_ptr() as *const u8, idx.len() * 4) };
        let (b, o) = sub.stage_bytes(g, bytes);
        unsafe {
            dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT32);
            dev.cmd_draw_indexed(sub.main, idx.len() as u32, inst, 0, d.base_vertex, d.base_instance as u32);
        }
        return;
    }
    let ity = match d.itype {
        index_type::U8 if g.d.caps.index_u8 => vk::IndexType::UINT8_EXT,
        index_type::U16 => vk::IndexType::UINT16,
        index_type::U32 => vk::IndexType::UINT32,
        _ => {
            // u8 indices widened to u16 on the CPU
            let src = unsafe { core::slice::from_raw_parts(d.ibuf as *const u8, d.count.max(0) as usize) };
            let w: Vec<u16> = src.iter().map(|&b| b as u16).collect();
            let bytes = unsafe { core::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2) };
            let (b, o) = sub.stage_bytes(g, bytes);
            unsafe {
                dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT16);
                dev.cmd_draw_indexed(sub.main, d.count as u32, inst, 0, d.base_vertex, d.base_instance as u32);
            }
            return;
        }
    };
    if qs.prim_restart.0 {
        let all = match d.itype {
            index_type::U8 => 0xff,
            index_type::U16 => 0xffff,
            _ => u32::MAX,
        };
        if qs.prim_restart.1 != all && qs.prim_restart.1 != u32::MAX {
            crate::log::red_once("primitive restart with an index other than all ones");
        }
    }
    let n = if d.ind != 0 { 1u64 << 20 } else { d.count.max(0) as u64 * isz };
    let Some((ib, io, _)) = super::mem::resolve(g, sub, d.ibuf, n.max(1).min(1 << 20)) else { return };
    unsafe {
        dev.cmd_bind_index_buffer(sub.main, ib, io, ity);
        if d.ind != 0 {
            if let Some((b, o, _)) = super::mem::resolve(g, sub, d.ind, 20) {
                dev.cmd_draw_indexed_indirect(sub.main, b, o, 1, 20);
            }
        } else {
            dev.cmd_draw_indexed(sub.main, d.count as u32, inst, 0, d.base_vertex, d.base_instance as u32);
        }
    }
}

fn dispatch(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_dispatch) {
    end_pass(g, sub, qs);
    let Some(cs) = qs.prog[5].clone() else {
        crate::log::red_once("dispatch with no compute shader bound");
        return;
    };
    let Some(pipe) = g.compute_pipeline(&cs) else { return };
    unsafe { g.dev().cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, pipe) };
    bind_sets(g, sub, qs, std::slice::from_ref(&cs), vk::PipelineBindPoint::COMPUTE, [d.x as u32, d.y as u32, d.z as u32], 0);
    push_constants(g, sub, qs, false);
    unsafe {
        if d.ind != 0 {
            if let Some((b, o, _)) = super::mem::resolve(g, sub, d.ind, 12) {
                g.dev().cmd_dispatch_indirect(sub.main, b, o);
            }
        } else {
            g.dev().cmd_dispatch(sub.main, d.x.max(0) as u32, d.y.max(0) as u32, d.z.max(0) as u32);
        }
    }
    barrier(g, sub);
}

fn clear_rect(qs: &QState) -> vk::ClearRect {
    let full = vk::Rect2D { offset: vk::Offset2D::default(), extent: qs.pass_extent };
    let rect = match qs.scissor {
        Some([x, y, w, h]) => {
            let x0 = x.max(0) as u32;
            let y0 = y.max(0) as u32;
            let x1 = ((x + w).max(0) as u32).min(qs.pass_extent.width);
            let y1 = ((y + h).max(0) as u32).min(qs.pass_extent.height);
            vk::Rect2D { offset: vk::Offset2D { x: x0 as i32, y: y0 as i32 }, extent: vk::Extent2D { width: x1.saturating_sub(x0), height: y1.saturating_sub(y0) } }
        }
        None => full,
    };
    vk::ClearRect { rect, base_array_layer: 0, layer_count: 1 }
}

fn region_copy(t: &tex_t, view: Option<&view_t>, r: &NVNcopyRegion, tf: &format::TexFormat) -> (vk::ImageSubresourceLayers, vk::Offset3D, vk::Extent3D) {
    let level = view.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
    let layer0 = view.filter(|v| v.bits & VW_LAYERS != 0).map(|v| v.min_layer as u32).unwrap_or(0);
    let is3d = t.d.target == target::T3D;
    (
        vk::ImageSubresourceLayers {
            aspect_mask: super::image::upload_aspect(tf),
            mip_level: level,
            base_array_layer: if is3d { layer0 } else { layer0 + r.zoffset.max(0) as u32 },
            layer_count: if is3d { 1 } else { r.depth.max(1) as u32 },
        },
        vk::Offset3D { x: r.xoffset, y: r.yoffset, z: if is3d { r.zoffset } else { 0 } },
        vk::Extent3D { width: r.width.max(1) as u32, height: r.height.max(1) as u32, depth: if is3d { r.depth.max(1) as u32 } else { 1 } },
    )
}

/// A buffer <-> texture copy's buffer layout (texels per row, rows per image) and byte size.
fn buffer_layout(t: &tex_t, r: &NVNcopyRegion, strides: Option<p_copy_strides>) -> (u32, u32, u64) {
    let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
    let tight_row = (r.width.max(1) as u32).div_ceil(bw) as u64 * bpb as u64;
    let rows = (r.height.max(1) as u32).div_ceil(bh) as u64;
    let s = strides.unwrap_or_default();
    let row = if s.row > 0 { s.row as u64 } else { tight_row };
    let img = if s.image > 0 { s.image as u64 } else { row * rows };
    let row_texels = if s.row > 0 && bpb > 0 { (row / bpb as u64) as u32 * bw } else { 0 };
    let img_rows = if s.image > 0 && row > 0 { (img / row) as u32 * bh } else { 0 };
    (row_texels, img_rows, img * r.depth.max(1) as u64)
}

fn rd_payload<T: Copy>(rec: &[u8]) -> T {
    // SAFETY: payload structs are plain data for which all-zero is valid (short records read as zero-extended).
    let mut t: T = unsafe { core::mem::zeroed() };
    let n = core::mem::size_of::<T>().min(rec.len() - 8);
    unsafe { core::ptr::copy_nonoverlapping(rec.as_ptr().add(8), &mut t as *mut T as *mut u8, n) };
    t
}
unsafe fn rd_at<T: Copy>(rec: &[u8], off: usize) -> T {
    core::ptr::read_unaligned(rec.as_ptr().add(8 + off) as *const T)
}

/// Whether the wait at the start of a parked stream now passes.
pub fn wait_passes(g: &Gpu, stream: &[u8]) -> bool {
    if stream.len() < 8 {
        return true;
    }
    let h = unsafe { rd::<op_hdr>(stream.as_ptr()) };
    if h.op != op::WAIT_EVENT {
        return true;
    }
    let (addr, mode, value): (u64, u32, u32) = unsafe { (rd_at(stream, 0), rd_at(stream, 8), rd_at(stream, 12)) };
    event_wait_passes(g, addr, mode, value)
}

fn event_wait_passes(g: &Gpu, addr: u64, mode: u32, value: u32) -> bool {
    let cur = unsafe { (*(addr as *const std::sync::atomic::AtomicU32)).load(std::sync::atomic::Ordering::Acquire) };
    if crate::nvn::sync::event_passes(cur, mode, value) {
        return true;
    }
    // a GPU WRITE signal encoded before and still in flight releases it: the queue runs it first
    matches!(g.q.pending_signals.lock().unwrap().get(&addr), Some(&(m, v, _)) if m == event_signal::WRITE && crate::nvn::sync::event_passes(v, mode, value))
}

/// Replays `stream` (canonical records). Returns the offset of a wait that parks the queue, if any.
pub fn run(g: &Gpu, sub: &mut Sub, qs: &mut QState, stream: &[u8]) -> Option<usize> {
    let mut p = 0usize;
    while p + 8 <= stream.len() {
        let h = unsafe { rd::<op_hdr>(stream.as_ptr().add(p)) };
        let sz = h.size as usize;
        if sz < 8 || p + sz > stream.len() {
            crate::log::red("replay: a record does not fit its stream");
            break;
        }
        let rec = &stream[p..p + sz];
        if !record(g, sub, qs, h.op, rec) {
            return Some(p);
        }
        p += sz;
    }
    None
}

/// One record; false = this wait parks the queue.
fn record(g: &Gpu, sub: &mut Sub, qs: &mut QState, opc: u32, rec: &[u8]) -> bool {
    let dev = g.dev();
    match opc {
        op::END => {}
        op::SET_RT => {
            end_pass(g, sub, qs);
            let r: p_set_rt = rd_payload(rec);
            qs.rt_n = r.n.clamp(0, 8) as usize;
            for i in 0..8 {
                qs.rt[i] = Rt { tex: None, view: None };
                if i < qs.rt_n && !r.color[i].is_null() {
                    let t = unsafe { *r.color[i] };
                    if t.valid() {
                        qs.rt[i] = Rt { tex: Some(t), view: if r.has_cview[i] != 0 { Some(r.cview[i]) } else { None } };
                    }
                }
            }
            qs.rt_depth = Rt { tex: None, view: None };
            if !r.depth.is_null() {
                let t = unsafe { *r.depth };
                if t.valid() {
                    qs.rt_depth = Rt { tex: Some(t), view: if r.has_dview != 0 { Some(r.dview) } else { None } };
                }
            }
        }
        op::CLEAR_COLOR => {
            let c: p_clear_color = rd_payload(rec);
            let i = c.index.clamp(0, 7) as usize;
            if i >= qs.rt_n || qs.rt[i].tex.is_none() || !begin_pass(g, sub, qs) {
                return true;
            }
            if c.mask & 0xf != 0xf {
                crate::log::red_once("ClearColor with a partial channel mask clears every channel");
            }
            let kind = qs.rt[i].tex.and_then(|t| format::tex_format(t.d.format)).map(|f| f.kind).unwrap_or(Kind::Float);
            let v = match kind {
                Kind::Uint => vk::ClearColorValue { uint32: c.rgba.map(|x| x as u32) },
                Kind::Sint => vk::ClearColorValue { int32: c.rgba.map(|x| x as i32) },
                _ => vk::ClearColorValue { float32: c.rgba },
            };
            let a = [vk::ClearAttachment { aspect_mask: vk::ImageAspectFlags::COLOR, color_attachment: i as u32, clear_value: vk::ClearValue { color: v } }];
            let r = clear_rect(qs);
            if r.rect.extent.width > 0 && r.rect.extent.height > 0 {
                unsafe { dev.cmd_clear_attachments(sub.main, &a, &[r]) };
            }
        }
        op::CLEAR_DS => {
            let c: p_clear_ds = rd_payload(rec);
            let Some(t) = qs.rt_depth.tex else { return true };
            if !begin_pass(g, sub, qs) {
                return true;
            }
            let kind = format::tex_format(t.d.format).map(|f| f.kind).unwrap_or(Kind::Depth);
            let mut asp = vk::ImageAspectFlags::empty();
            if c.depth_mask != 0 && kind != Kind::Stencil {
                asp |= vk::ImageAspectFlags::DEPTH;
            }
            if c.stencil_mask != 0 && matches!(kind, Kind::DepthStencil | Kind::Stencil) {
                asp |= vk::ImageAspectFlags::STENCIL;
            }
            if !asp.is_empty() {
                let a = [vk::ClearAttachment {
                    aspect_mask: asp,
                    color_attachment: 0,
                    clear_value: vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: c.depth.clamp(0.0, 1.0), stencil: c.stencil as u32 } },
                }];
                let r = clear_rect(qs);
                if r.rect.extent.width > 0 && r.rect.extent.height > 0 {
                    unsafe { dev.cmd_clear_attachments(sub.main, &a, &[r]) };
                }
            }
        }
        op::VIEWPORTS => {
            let v: p_viewports = rd_payload(rec);
            if v.first == 0 {
                qs.vp_origin = v.v[0][4].to_bits();
            }
            for i in 0..v.n.clamp(0, 16) as usize {
                let k = v.first as usize + i;
                if k < 16 {
                    qs.vp[k] = [v.v[i][0], v.v[i][1], v.v[i][2], v.v[i][3]];
                }
            }
        }
        op::SCISSOR => {
            let s: p_scissor = rd_payload(rec);
            qs.scissor = Some([s.x, s.y, s.w, s.h]);
        }
        op::DEPTH_RANGE => {
            let d: p_depth_range = rd_payload(rec);
            qs.depth_range = [d.n, d.f];
        }
        op::BIND_PROGRAM => {
            let b: p_bind_program = rd_payload(rec);
            if b.prog.is_null() {
                for s in 0..6 {
                    if b.stages & (1 << s) != 0 {
                        qs.prog[s] = None;
                    }
                }
                return true;
            }
            let pr = unsafe { &*b.prog };
            if pr.magic != MAGIC_PROG {
                crate::log::red_once("BindProgram of a finalized program");
                return true;
            }
            let keys = unsafe { crate::nvn::prog::prog_keys(pr) };
            for s in 0..6 {
                if b.stages & (1 << s) != 0 {
                    qs.prog[s] = None;
                }
            }
            for k in keys {
                if let Some(m) = g.shaders.get(dev, &k) {
                    let s = m.stage as usize;
                    if b.stages & (1 << s) != 0 {
                        qs.prog[s] = Some(m);
                    }
                }
            }
        }
        op::BIND_STATE => {
            let b: p_bind_state = rd_payload(rec);
            let u32_ = u32::from_le_bytes(b.bytes[..4].try_into().unwrap());
            let u64_ = u64::from_le_bytes(b.bytes[..8].try_into().unwrap());
            match b.kind {
                0 => {
                    let bs = blend_st(u64_);
                    qs.blend[bs.get(blend_field::TARGET) as usize & 7] = bs;
                }
                1 => qs.color = color_st(u32_),
                2 => qs.chmask = u32_,
                3 => qs.ms = unsafe { core::ptr::read_unaligned(b.bytes.as_ptr() as *const ms_st) },
                4 => qs.poly = poly_st(u32_),
                5 => qs.ds = ds_st(u64_),
                _ => {}
            }
        }
        op::BIND_VATTR => {
            let n: i32 = unsafe { rd_at(rec, 4) };
            let n = n.clamp(0, 32) as usize;
            for i in 0..n {
                qs.vattr[i] = unsafe { rd_at(rec, 8 + 4 * i) };
            }
            qs.nvattr = n;
        }
        op::BIND_VSTREAM => {
            let n: i32 = unsafe { rd_at(rec, 4) };
            for i in 0..n.clamp(0, 16) as usize {
                qs.vstream[i] = unsafe { rd_at(rec, 8 + 8 * i) };
            }
        }
        op::BIND_VBUF => {
            let first: i32 = unsafe { rd_at(rec, 0) };
            let n: i32 = unsafe { rd_at(rec, 4) };
            for i in 0..n.clamp(0, 16) as usize {
                let r: NVNbufferRange = unsafe { rd_at(rec, 8 + 16 * i) };
                let k = first as usize + i;
                if k < 16 {
                    qs.vbuf[k] = (r.address, r.size);
                }
            }
        }
        op::BIND_VBUF1 => {
            let b: p_bind_vbuf1 = rd_payload(rec);
            if (b.index as usize) < 16 {
                qs.vbuf[b.index as usize] = (b.addr, b.size as u64);
            }
        }
        op::BIND_UBO | op::BIND_UBO_SMALL => {
            let b: p_bind_buf = if opc == op::BIND_UBO {
                rd_payload(rec)
            } else {
                let s: p_bind_ubo_small = rd_payload(rec);
                p_bind_buf { stage: s.stage as i32, index: s.index as i32, addr: s.addr, size: s.size as u64 }
            };
            if (b.stage as usize) < 7 && (b.index as usize) < 16 {
                qs.ubo[b.stage as usize][b.index as usize] = (b.addr, b.size);
            }
        }
        op::BIND_SSBO => {
            let b: p_bind_buf = rd_payload(rec);
            if (b.stage as usize) < 7 && (b.index as usize) < 16 {
                qs.ssbo[b.stage as usize][b.index as usize] = (b.addr, b.size);
            }
        }
        op::BIND_TEX | op::BIND_SAMP | op::BIND_IMG => {
            let (stage, first, n): (i32, i32, i32) = unsafe { (rd_at(rec, 0), rd_at(rec, 4), rd_at(rec, 8)) };
            if (stage as usize) < 7 {
                for i in 0..n.clamp(0, 32) as usize {
                    let h: u64 = unsafe { rd_at(rec, 24 + 8 * i) };
                    let k = first as usize + i;
                    match opc {
                        op::BIND_TEX if k < 256 => qs.tex[stage as usize][k] = h as u32,
                        op::BIND_SAMP if k < 128 => qs.samp[stage as usize][k] = h as u32,
                        op::BIND_IMG if k < 32 => qs.img[stage as usize][k] = h as u32,
                        _ => {}
                    }
                }
            }
        }
        op::BIND_TEX1 => {
            let b: p_bind_tex1 = rd_payload(rec);
            if (b.stage as usize) < 7 && (b.slot as usize) < 256 {
                qs.tex[b.stage as usize][b.slot as usize] = b.h as u32;
            }
        }
        op::SET_TEX_POOL => qs.tex_pool = rd_payload::<usize>(rec),
        op::SET_SAMP_POOL => qs.samp_pool = rd_payload::<usize>(rec),
        op::STENCIL => {
            let s: p_stencil = rd_payload(rec);
            let v = match s.which {
                0 => s.value_mask,
                1 => s.mask,
                _ => s.r#ref,
            } as u32;
            for f in 0..2 {
                if s.face as u32 & (1 << f) != 0 {
                    qs.stencil[f][s.which.clamp(0, 2) as usize] = v;
                }
            }
        }
        op::POLY_OFFSET => {
            let o: p_poly_offset = rd_payload(rec);
            qs.poly_off = [o.factor, o.units, o.clamp];
        }
        op::BLEND_COLOR => qs.blend_color = rd_payload::<p_blend_color>(rec).rgba,
        op::PRIM_RESTART => {
            let r: p_prim_restart = rd_payload(rec);
            qs.prim_restart = (r.on != 0, r.index);
        }
        op::LINE_WIDTH => qs.line_width = rd_payload::<p_line_width>(rec).w,
        op::DEPTH_CLAMP => qs.depth_clamp = rd_payload::<p_depth_clamp>(rec).on != 0,
        op::RAST_DISCARD => qs.rast_discard = rd_payload::<p_rast_discard>(rec).on != 0,
        op::PATCH_SIZE => qs.patch = rd_payload::<p_patch_size>(rec).n.max(1) as u32,
        op::DRAW => draw(g, sub, qs, rd_payload(rec)),
        op::DRAW_IDX => draw_idx(g, sub, qs, rd_payload(rec)),
        op::DISPATCH => dispatch(g, sub, qs, rd_payload(rec)),
        op::BARRIER => {
            end_pass(g, sub, qs);
            barrier(g, sub);
        }
        op::COPY_B2B => {
            end_pass(g, sub, qs);
            let [src, dst, n]: [u64; 3] = unsafe { rd_at(rec, 0) };
            if let (Some((sb, so, _)), Some((db, doff, _))) = (super::mem::resolve(g, sub, src, n), super::mem::resolve(g, sub, dst, n)) {
                unsafe { dev.cmd_copy_buffer(sub.main, sb, db, &[vk::BufferCopy { src_offset: so, dst_offset: doff, size: n }]) };
                super::mem::note_gpu_write(g, sub, dst, n);
                barrier(g, sub);
            }
        }
        op::UPDATE_UB => {
            end_pass(g, sub, qs);
            let [dst, n]: [u64; 2] = unsafe { rd_at(rec, 0) };
            let data = &rec[24..24 + n as usize];
            if let Some((db, doff, _)) = super::mem::resolve(g, sub, dst, n) {
                let (sb, so) = sub.stage_bytes(g, data);
                unsafe { dev.cmd_copy_buffer(sub.main, sb, db, &[vk::BufferCopy { src_offset: so, dst_offset: doff, size: n }]) };
                super::mem::note_gpu_write(g, sub, dst, n);
                barrier(g, sub);
            }
        }
        op::CLEAR_BUF => {
            end_pass(g, sub, qs);
            let c: p_clear_buf = rd_payload(rec);
            if let Some((b, o, _)) = super::mem::resolve(g, sub, c.addr, c.size) {
                unsafe { dev.cmd_fill_buffer(sub.main, b, o, c.size & !3, c.value) };
                super::mem::note_gpu_write(g, sub, c.addr, c.size);
                barrier(g, sub);
            }
        }
        op::COPY_B2T | op::COPY_T2B => {
            end_pass(g, sub, qs);
            let c: p_copy_b2t = rd_payload(rec);
            if c.tex.is_null() {
                return true;
            }
            let t = unsafe { *c.tex };
            let strides = (rec.len() - 8 >= COPY_STRIDES_OFF + 16).then(|| unsafe { rd_at::<p_copy_strides>(rec, COPY_STRIDES_OFF) });
            let (row, img_rows, bytes) = buffer_layout(&t, &c.r, strides);
            let view = (c.has_view != 0).then_some(c.view);
            let img = if opc == op::COPY_B2T { target_image(g, sub, &t).map(|x| (x.0, x.1)) } else { source_image(g, sub, &t).map(|x| (x.0, x.1)) };
            let Some((image, tf)) = img else { return true };
            if tf.expand3 != 0 || tf.astc.is_some() {
                crate::log::red_once("buffer <-> texture copy of an RGB or ASTC texture is not converted yet");
                return true;
            }
            let Some((b, o, _)) = super::mem::resolve(g, sub, c.src, bytes) else { return true };
            let (sl, off, ext) = region_copy(&t, view.as_ref(), &c.r, &tf);
            let r = vk::BufferImageCopy { buffer_offset: o, buffer_row_length: row, buffer_image_height: img_rows, image_subresource: sl, image_offset: off, image_extent: ext };
            unsafe {
                if opc == op::COPY_B2T {
                    dev.cmd_copy_buffer_to_image(sub.main, b, image, vk::ImageLayout::GENERAL, &[r]);
                } else {
                    dev.cmd_copy_image_to_buffer(sub.main, image, vk::ImageLayout::GENERAL, b, &[r]);
                }
            }
            if opc == op::COPY_T2B {
                super::mem::note_gpu_write(g, sub, c.src, bytes);
            }
            barrier(g, sub);
        }
        op::COPY_T2T => {
            end_pass(g, sub, qs);
            let c: p_copy_t2t = rd_payload(rec);
            if c.src.is_null() || c.dst.is_null() {
                return true;
            }
            let (s, d) = unsafe { (*c.src, *c.dst) };
            let Some((si, stf, _)) = source_image(g, sub, &s) else { return true };
            let Some((di, dtf, _, _)) = target_image(g, sub, &d) else { return true };
            let (ssl, soff, sext) = region_copy(&s, (c.has_sv != 0).then_some(&c.sv), &c.sr, &stf);
            let (dsl, doff, dext) = region_copy(&d, (c.has_dv != 0).then_some(&c.dv), &c.dr, &dtf);
            unsafe {
                if sext == dext && stf.vk == dtf.vk {
                    let r = vk::ImageCopy { src_subresource: ssl, src_offset: soff, dst_subresource: dsl, dst_offset: doff, extent: sext };
                    dev.cmd_copy_image(sub.main, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &[r]);
                } else {
                    let end = |o: vk::Offset3D, e: vk::Extent3D| vk::Offset3D { x: o.x + e.width as i32, y: o.y + e.height as i32, z: o.z + e.depth as i32 };
                    let r = vk::ImageBlit { src_subresource: ssl, src_offsets: [soff, end(soff, sext)], dst_subresource: dsl, dst_offsets: [doff, end(doff, dext)] };
                    let filter = if c.flags & 1 != 0 { vk::Filter::LINEAR } else { vk::Filter::NEAREST };
                    dev.cmd_blit_image(sub.main, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &[r], filter);
                }
            }
            barrier(g, sub);
        }
        op::CLEAR_TEX => {
            end_pass(g, sub, qs);
            let c: p_clear_tex = rd_payload(rec);
            if c.tex.is_null() {
                return true;
            }
            let t = unsafe { *c.tex };
            let Some((image, tf, ext, _)) = target_image(g, sub, &t) else { return true };
            let view = (c.has_view != 0).then_some(c.view);
            let (sl, off, e) = region_copy(&t, view.as_ref(), &c.r, &tf);
            if off.x != 0 || off.y != 0 || e.width < (ext.width >> sl.mip_level).max(1) || e.height < (ext.height >> sl.mip_level).max(1) {
                crate::log::red_once("ClearTexture of part of a level clears the whole level");
            }
            let range = vk::ImageSubresourceRange { aspect_mask: sl.aspect_mask, base_mip_level: sl.mip_level, level_count: 1, base_array_layer: sl.base_array_layer, layer_count: sl.layer_count };
            unsafe {
                if matches!(tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                    let v = vk::ClearDepthStencilValue { depth: f32::from_bits(c.c[0]), stencil: c.c[1] };
                    dev.cmd_clear_depth_stencil_image(sub.main, image, vk::ImageLayout::GENERAL, &v, &[range]);
                } else {
                    let v = if c.is_uint != 0 { vk::ClearColorValue { uint32: c.c } } else { vk::ClearColorValue { float32: c.c.map(f32::from_bits) } };
                    dev.cmd_clear_color_image(sub.main, image, vk::ImageLayout::GENERAL, &v, &[range]);
                }
            }
            barrier(g, sub);
        }
        op::SIGNAL_EVENT => {
            let (addr, mode, value): (u64, u32, u32) = unsafe { (rd_at(rec, 0), rd_at(rec, 8), rd_at(rec, 12)) };
            sub.post.push(Post::Event { addr, mode, value });
            let mut ps = g.q.pending_signals.lock().unwrap();
            if mode == event_signal::WRITE {
                ps.insert(addr, (mode, value, g.q.last_fence() + 1));
            } else {
                ps.remove(&addr);
            }
        }
        op::WAIT_EVENT => {
            let (addr, mode, value): (u64, u32, u32) = unsafe { (rd_at(rec, 0), rd_at(rec, 8), rd_at(rec, 12)) };
            if !event_wait_passes(g, addr, mode, value) {
                end_pass(g, sub, qs);
                return false;
            }
        }
        op::WAIT_SYNC => {}
        op::FENCE_SYNC => {
            let s: u64 = unsafe { rd_at(rec, 0) };
            if s != 0 {
                let so = s as *mut sync_t;
                unsafe {
                    (*so).fenced = 1;
                    (*so).fence = g.q.last_fence() + 1;
                }
            }
        }
        op::REPORT_COUNTER => {
            let (kind, addr): (u64, u64) = unsafe { (rd_at(rec, 0), rd_at(rec, 8)) };
            if kind as u32 != counter::TIMESTAMP && kind as u32 != counter::TIMESTAMP_TOP {
                crate::log::red_once(&format!("counter report of type {kind}: only timestamps are counted (others read 0)"));
            }
            let q = sub.query();
            if q < 256 {
                unsafe { dev.cmd_write_timestamp2(sub.main, vk::PipelineStageFlags2::ALL_COMMANDS, sub.query, q) };
                sub.post.push(Post::Counter { addr, kind: kind as u32, query: q });
            }
        }
        op::RESET_COUNTER => {}
        _ => crate::log::red_once(&format!("replay: record op {opc} is not implemented yet")),
    }
    true
}

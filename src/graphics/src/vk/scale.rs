//! Resolution scale: the ini's `resolution` (0.25 .. 4, live) and the host's dynamic resolution (`dynamic_resolution`,
//! down to `dynamic_resolution_min` while the GPU cannot keep up), on the Vulkan replay.
//!
//! - **What is scaled:** render targets: compressible textures (NVN's flag for colour and depth targets) that are 2D
//!   (or 2D arrays), not storage images, not block-compressed, and more than one texel in both directions (one-row /
//!   one-column targets hold data such as exposure, not pictures). Their images are `scale` times the texture's size;
//!   everything else stays at the game's size.
//! - **Passes:** a pass's viewports, scissors and clear rectangles are multiplied by its targets' scale.
//! - **Shaders:** the translator (Ryujinx's design) divides gl_FragCoord by the support buffer's `render_scale[0]` and
//!   multiplies the integer coordinates of texel fetches, image loads and texture sizes of each texture binding by
//!   `render_scale[1 + k]` (negative in a pixel shader: the fetch keeps the pixel's own sub-texel). The replay fills
//!   them per stage from the bound textures' images.
//! - **Copies** between images of different scales are blits; buffer <-> texture copies and texel reads of a scaled
//!   image go through a bounce image at the game's size; a scaled target the CPU fills is uploaded the same way.
//! - **Live changes** (the option, the dynamic resolution): at the start of the next submission every scaled image is
//!   made again at the new scale, its content blitted over, and the texture pool's descriptors written again.
//! - Presents and screenshots crop the scaled picture by the same factor.

use super::Gpu;
use crate::nvn::obj::tex_t;
use crate::nvn::types::{target, tex_flags};
use ash::vk;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

/// The scale new and rescaled images get (f32 bits), and whether any image has a scale other than 1.
static CUR: AtomicU32 = AtomicU32::new(0x3f80_0000);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// the dynamic resolution's factor (f32 bits, <= 1) over the ini's scale
static DRS: AtomicU32 = AtomicU32::new(0x3f80_0000);

fn f(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

/// The scale images are made at now.
pub fn current() -> f32 {
    f(&CUR)
}

/// Whether any image is scaled (the replay skips all scale work otherwise).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// What the options ask for now: the ini's scale times the dynamic resolution's factor, in steps of 1/20.
fn wanted() -> f32 {
    let ini = {
        let v = nxconfig::f32("resolution");
        if v > 0.0 {
            v.clamp(0.25, 4.0)
        } else {
            1.0
        }
    };
    let s = if nxconfig::flag("dynamic_resolution") {
        ini * f(&DRS)
    } else {
        ini
    };
    ((s * 20.0).round() / 20.0).clamp(0.25, 4.0)
}

/// A texture whose image is scaled (see the module's notes).
pub fn candidate(t: &tex_t) -> bool {
    let d = &t.d;
    let fl = d.flags as u32;
    fl & tex_flags::COMPRESSIBLE != 0
        && fl
            & (tex_flags::IMAGE
                | tex_flags::LINEAR
                | tex_flags::LINEAR_RENDER_TARGET
                | tex_flags::SPARSE)
            == 0
        && matches!(
            d.target,
            target::T2D
                | target::T2D_ARRAY
                | target::T2D_MS
                | target::T2D_MS_ARRAY
                | target::RECTANGLE
        )
        && d.width > 1
        && d.height > 1
        && crate::tex::layout::fmt_bpb(d.format).1 == 1
}

/// A size at scale `s` (at least 1).
pub fn dim(v: u32, s: f32) -> u32 {
    ((v as f32 * s + 0.5) as u32).max(1)
}
/// A coordinate at scale `s`.
pub fn coord(v: i32, s: f32) -> i32 {
    (v as f32 * s).round() as i32
}

/// The scale a new image of `t` gets.
pub fn for_new(t: &tex_t) -> f32 {
    let s = current();
    if s != 1.0 && candidate(t) {
        ACTIVE.store(true, Ordering::Relaxed);
        s
    } else {
        1.0
    }
}

/// The blit filter between two images of a format: linear for filterable float colour, nearest otherwise.
pub fn filter(g: &Gpu, fmt: vk::Format, kind: crate::tex::format::Kind) -> vk::Filter {
    use crate::tex::format::Kind;
    let lin =
        g.d.format_features(fmt)
            .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR);
    if lin && matches!(kind, Kind::Float) {
        vk::Filter::LINEAR
    } else {
        vk::Filter::NEAREST
    }
}

/// A transient image of the game's size for copies in and out of a scaled image: destroyed once the GPU is done with it.
pub fn bounce(g: &Gpu, fmt: vk::Format, e: vk::Extent3D, layers: u32) -> Option<vk::Image> {
    let ci = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(fmt)
        .extent(e)
        .mip_levels(1)
        .array_layers(layers.max(1))
        .samples(vk::SampleCountFlags::TYPE_1)
        .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let img = unsafe { g.dev().create_image(&ci, None) }.ok()?;
    let req = unsafe { g.dev().get_image_memory_requirements(img) };
    let a = g
        .alloc
        .lock()
        .unwrap()
        .allocate(&gpu_allocator::vulkan::AllocationCreateDesc {
            name: "scale bounce",
            requirements: req,
            location: gpu_allocator::MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: gpu_allocator::vulkan::AllocationScheme::GpuAllocatorManaged,
        })
        .ok()?;
    unsafe { g.dev().bind_image_memory(img, a.memory(), a.offset()) }.ok()?;
    let a = Mutex::new(Some(a));
    g.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
        g.dev().destroy_image(img, None);
        if let Some(a) = a.lock().unwrap().take() {
            let _ = g.alloc.lock().unwrap().free(a);
        }
    }));
    Some(img)
}

/// Barriers from `UNDEFINED` / transfer to `GENERAL` for a bounce image and between the steps of a bounce.
pub fn general(
    g: &Gpu,
    cb: vk::CommandBuffer,
    img: vk::Image,
    aspect: vk::ImageAspectFlags,
    layers: u32,
    from_undefined: bool,
) {
    let b = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
        .old_layout(if from_undefined {
            vk::ImageLayout::UNDEFINED
        } else {
            vk::ImageLayout::GENERAL
        })
        .new_layout(vk::ImageLayout::GENERAL)
        .image(img)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: aspect,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: layers.max(1),
        })];
    unsafe {
        g.dev()
            .cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&b))
    };
}

/// A blit of a region given in the game's texels between images of scales `ss` and `ds`.
#[allow(clippy::too_many_arguments)]
pub fn blit(
    g: &Gpu,
    cb: vk::CommandBuffer,
    src: vk::Image,
    ss: f32,
    sl: vk::ImageSubresourceLayers,
    so: vk::Offset3D,
    dst: vk::Image,
    ds: f32,
    dl: vk::ImageSubresourceLayers,
    d_o: vk::Offset3D,
    e: vk::Extent3D,
    filter: vk::Filter,
) {
    let at = |o: vk::Offset3D, s: f32| vk::Offset3D {
        x: coord(o.x, s),
        y: coord(o.y, s),
        z: o.z,
    };
    let end = |o: vk::Offset3D, s: f32| vk::Offset3D {
        x: coord(o.x + e.width as i32, s),
        y: coord(o.y + e.height as i32, s),
        z: o.z + e.depth as i32,
    };
    let r = vk::ImageBlit {
        src_subresource: sl,
        src_offsets: [at(so, ss), end(so, ss)],
        dst_subresource: dl,
        dst_offsets: [at(d_o, ds), end(d_o, ds)],
    };
    unsafe {
        g.dev().cmd_blit_image(
            cb,
            src,
            vk::ImageLayout::GENERAL,
            dst,
            vk::ImageLayout::GENERAL,
            &[r],
            filter,
        )
    };
}

// ---------- the host's dynamic resolution ----------
static DRS_T0: AtomicU64 = AtomicU64::new(0);
static DRS_FRAMES: AtomicU64 = AtomicU64::new(0);
static DRS_GPU_NS: AtomicU64 = AtomicU64::new(0);
struct DrsState {
    hold: u32,
    up: u32,
    last_down: Option<std::time::Instant>,
}
static DRS_ST: Mutex<DrsState> = Mutex::new(DrsState {
    hold: 0,
    up: 0,
    last_down: None,
});

/// A submission's GPU time (the completion thread).
pub fn note_gpu(ns: u64) {
    DRS_GPU_NS.fetch_add(ns, Ordering::Relaxed);
}

/// At every present: once a second, the dynamic resolution steps down 0.1 when the GPU is busy over 90% of the time,
/// and up 0.1 after four seconds in a row under 65% and 20 s after the last step down (frames that make
/// pipelines do not count).
pub fn frame() {
    DRS_FRAMES.fetch_add(1, Ordering::Relaxed);
    if !nxconfig::flag("dynamic_resolution") {
        return;
    }
    let now = crate::nvn::device::now_ns();
    let t0 = DRS_T0.load(Ordering::Relaxed);
    if t0 == 0 {
        DRS_T0.store(now, Ordering::Relaxed);
        DRS_GPU_NS.store(0, Ordering::Relaxed);
        return;
    }
    let dt = now.saturating_sub(t0);
    if dt < 1_000_000_000 {
        return;
    }
    DRS_T0.store(now, Ordering::Relaxed);
    let busy = DRS_GPU_NS.swap(0, Ordering::Relaxed) as f64 / dt as f64;
    static PIPES: AtomicU64 = AtomicU64::new(0);
    let pipes =
        super::perf::get(super::perf::C::PipesGfx) + super::perf::get(super::perf::C::PipesCompute);
    let new_pipes = pipes != PIPES.swap(pipes, Ordering::Relaxed);
    let mut st = DRS_ST.lock().unwrap();
    if new_pipes {
        st.hold = 2;
        return;
    }
    if st.hold > 0 {
        st.hold -= 1;
        return;
    }
    let min = {
        let v = nxconfig::f32("dynamic_resolution_min");
        if v > 0.0 {
            v.clamp(0.25, 1.0)
        } else {
            0.6
        }
    };
    let cur = f(&DRS);
    if busy > 0.90 && cur > min + 0.01 {
        DRS.store((cur - 0.1).max(min).to_bits(), Ordering::Relaxed);
        st.hold = 2;
        st.up = 0;
        st.last_down = Some(std::time::Instant::now());
        crate::vklog!(
            "dynamic resolution: the GPU was busy {:.0}% of the last second: x{:.2}",
            busy * 100.0,
            f(&DRS)
        );
    } else if busy < 0.65 && cur < 0.99 && st.last_down.is_none_or(|t| t.elapsed().as_secs() >= 20)
    {
        st.up += 1;
        if st.up >= 4 {
            DRS.store((cur + 0.1).min(1.0).to_bits(), Ordering::Relaxed);
            st.hold = 3;
            st.up = 0;
            crate::vklog!(
                "dynamic resolution: headroom ({:.0}% busy): x{:.2}",
                busy * 100.0,
                f(&DRS)
            );
        }
    } else {
        st.up = 0;
    }
}

/// At the start of a submission (the submission state held): a changed scale makes every scaled image again.
pub fn begin_submit(g: &Gpu, sub: &mut super::queue::Sub) {
    let want = wanted();
    if want == current() {
        return;
    }
    let old = current();
    CUR.store(want.to_bits(), Ordering::Relaxed);
    if want != 1.0 {
        ACTIVE.store(true, Ordering::Relaxed);
    }
    let n = g.rescale_images(sub, want);
    crate::vklog!("resolution scale x{old:.2} -> x{want:.2} ({n} render targets made again)");
}

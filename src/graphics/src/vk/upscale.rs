//! Sharp upscale at present (on by default): when the window is larger than the picture, the
//! picture is upscaled by a contrast-adaptive sharpening pass (`shaders/upscale.comp`) into an image of the window's
//! letterboxed size, which is then copied 1:1 into the swap chain image; otherwise the present's linear blit is used.
//! `NXGFX_UPSCALE=standard` (or the ini's `upscale=standard`) keeps the linear blit; `NXGFX_SHARPNESS` 0..1 (0.5).

use super::queue::Sub;
use super::Gpu;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use std::sync::Mutex;

static SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/upscale_comp.spv"));

/// The format of the upscaled picture (linear values: an sRGB source is decoded when sampled, encoded by the copy).
pub const FMT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

struct Target {
    image: vk::Image,
    view: vk::ImageView,
    alloc: Option<Allocation>,
    w: u32,
    h: u32,
}

struct Up {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipe: vk::Pipeline,
    sampler: vk::Sampler,
    targets: Vec<Target>,
    next: usize,
}

static UP: Mutex<Option<Up>> = Mutex::new(None);
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sharp (the default) or standard (the linear blit).
pub fn sharp() -> bool {
    match std::env::var("NXGFX_UPSCALE") {
        Ok(v) => v != "standard",
        Err(_) => nxconfig::string("upscale") != "standard",
    }
}

fn sharpness() -> f32 {
    std::env::var("NXGFX_SHARPNESS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.5f32)
        .clamp(0.0, 1.0)
}

fn words(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

impl Up {
    fn new(g: &Gpu) -> Result<Up, String> {
        let dev = g.dev();
        let binds = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let set_layout = unsafe {
            dev.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let pcr = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .size(48)];
        let sl = [set_layout];
        let layout = unsafe {
            dev.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sl)
                    .push_constant_ranges(&pcr),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let w = words(SPV);
        let m = unsafe {
            dev.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&w), None)
        }
        .map_err(|e| e.to_string())?;
        let st = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(m)
            .name(c"main");
        let pipe = unsafe {
            dev.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .stage(st)
                    .layout(layout)],
                None,
            )
        }
        .map_err(|(_, e)| e.to_string())?[0];
        unsafe { dev.destroy_shader_module(m, None) };
        let sci = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::NEAREST)
            .min_filter(vk::Filter::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .max_lod(0.0);
        let sampler = unsafe { dev.create_sampler(&sci, None) }.map_err(|e| e.to_string())?;
        crate::vklog!("present: upscaling to a window larger than the picture sharp (contrast-adaptive), sharpness {:.2} (NXGFX_UPSCALE=standard: bilinear)", sharpness());
        Ok(Up {
            set_layout,
            layout,
            pipe,
            sampler,
            targets: Vec::new(),
            next: 0,
        })
    }
}

fn make_target(g: &Gpu, w: u32, h: u32) -> Result<Target, String> {
    let dev = g.dev();
    let ci = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(FMT)
        .extent(vk::Extent3D {
            width: w,
            height: h,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe { dev.create_image(&ci, None) }.map_err(|e| e.to_string())?;
    let req = unsafe { dev.get_image_memory_requirements(image) };
    let a = g
        .alloc
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name: "sharp upscale",
            requirements: req,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| e.to_string())?;
    unsafe { dev.bind_image_memory(image, a.memory(), a.offset()) }.map_err(|e| e.to_string())?;
    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let view = unsafe {
        dev.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(FMT)
                .subresource_range(range),
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    Ok(Target {
        image,
        view,
        alloc: Some(a),
        w,
        h,
    })
}

/// Records the sharp upscale of `src` (format `fmt`)'s region `r` ([x0, y0, x1, y1], y0 > y1 turned over) to `w` x
/// `h` into an image of its own, left in GENERAL layout for a 1:1 copy. None: not wanted here (the window is not
/// larger, or sharp upscale is off or failed): the caller blits as before.
pub fn record(
    g: &Gpu,
    sub: &mut Sub,
    src: vk::Image,
    fmt: vk::Format,
    r: [i32; 4],
    w: u32,
    h: u32,
) -> Option<vk::Image> {
    let (sw, sh) = ((r[2] - r[0]).unsigned_abs(), (r[3] - r[1]).unsigned_abs());
    if w <= sw && h <= sh
        || w == 0
        || h == 0
        || !sharp()
        || FAILED.load(std::sync::atomic::Ordering::Relaxed)
    {
        return None;
    }
    if !g
        .d
        .format_features(fmt)
        .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
    {
        return None;
    }
    let mut lock = UP.lock().unwrap();
    if lock.is_none() {
        match Up::new(g) {
            Ok(u) => *lock = Some(u),
            Err(e) => {
                crate::log::red(&format!("sharp upscale: {e}; the linear blit is used"));
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                return None;
            }
        }
    }
    let up = lock.as_mut().unwrap();
    // targets of this size, used in turn (a present may still be reading the previous one)
    if up.targets.first().is_some_and(|t| t.w != w || t.h != h) {
        super::queue::wait_fence(g.q.last_fence(), u64::MAX);
        for t in up.targets.drain(..) {
            unsafe {
                g.dev().destroy_image_view(t.view, None);
                g.dev().destroy_image(t.image, None);
            }
            if let Some(a) = t.alloc {
                let _ = g.alloc.lock().unwrap().free(a);
            }
        }
    }
    let fresh = up.targets.is_empty();
    if fresh {
        for _ in 0..3 {
            match make_target(g, w, h) {
                Ok(t) => up.targets.push(t),
                Err(e) => {
                    crate::log::red(&format!(
                        "sharp upscale image: {e}; the linear blit is used"
                    ));
                    FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                    return None;
                }
            }
        }
    }
    let k = up.next;
    up.next = (k + 1) % up.targets.len();
    let (timg, tview) = (up.targets[k].image, up.targets[k].view);
    let dev = g.dev();
    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    // the source's view, released once this submission has run
    let sview = unsafe {
        dev.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(src)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(fmt)
                .subresource_range(range),
            None,
        )
    }
    .ok()?;
    g.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
        g.dev().destroy_image_view(sview, None)
    }));
    let b = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .src_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
        .dst_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::GENERAL)
        .image(timg)
        .subresource_range(range)];
    unsafe {
        dev.cmd_pipeline_barrier2(
            sub.main,
            &vk::DependencyInfo::default().image_memory_barriers(&b),
        )
    };
    super::queue::full_barrier(g, sub.main);
    let set = sub.alloc_set(g, up.set_layout);
    let si = [vk::DescriptorImageInfo {
        sampler: up.sampler,
        image_view: sview,
        image_layout: vk::ImageLayout::GENERAL,
    }];
    let oi = [vk::DescriptorImageInfo {
        sampler: vk::Sampler::null(),
        image_view: tview,
        image_layout: vk::ImageLayout::GENERAL,
    }];
    let wr = [
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&si),
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(&oi),
    ];
    unsafe { dev.update_descriptor_sets(&wr, &[]) };
    let pc: [f32; 12] = [
        r[0] as f32,
        r[1] as f32,
        r[2] as f32,
        r[3] as f32,
        w as f32,
        h as f32,
        0.0,
        0.0,
        sharpness(),
        0.0,
        0.0,
        0.0,
    ];
    let bytes = unsafe { core::slice::from_raw_parts(pc.as_ptr() as *const u8, 48) };
    unsafe {
        dev.cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, up.pipe);
        dev.cmd_bind_descriptor_sets(
            sub.main,
            vk::PipelineBindPoint::COMPUTE,
            up.layout,
            0,
            &[set],
            &[],
        );
        dev.cmd_push_constants(sub.main, up.layout, vk::ShaderStageFlags::COMPUTE, 0, bytes);
        dev.cmd_dispatch(sub.main, w.div_ceil(8), h.div_ceil(8), 1);
    }
    super::queue::full_barrier(g, sub.main);
    Some(timg)
}

//! The web device's Vulkan entry points (everything but `vkCmd*`, which is `cmd`, and submission, `submit`):
//! instance, the one physical device (the page's WebGPU adapter), memory, buffers, images, views, samplers, shader
//! modules (converted to WGSL), layouts, descriptor sets, pipelines, sync objects, queries, the canvas surface and its
//! swap chain. `lookup` names them for `vkGetInstanceProcAddr` / `vkGetDeviceProcAddr`.

use super::cmd;
use super::fmt;
use super::obj::{self, objs, op, Desc, Dsl, Gfx, Image, Memory, Pipeline, PipelineLayout, View};
use super::ring;
use super::submit;
use ash::vk;
use ash::vk::Handle;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::Ordering;
use std::sync::Arc;

type R = vk::Result;
const OK: R = vk::Result::SUCCESS;

// dispatchable handles: addresses of these statics
static INSTANCE: u8 = 0;
static PDEV: u8 = 0;
static DEVICE: u8 = 0;
static QUEUE: u8 = 0;

pub fn device_handle() -> vk::Device {
    vk::Device::from_raw(&DEVICE as *const u8 as u64)
}

/// The canvas surface's handle.
pub const SURFACE: u64 = 0x5f5f_5f01;

unsafe fn sl<'a, T>(p: *const T, n: u32) -> &'a [T] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}

/// Writes a Vulkan enumeration answer (count / array convention).
unsafe fn answer<T: Copy>(items: &[T], count: *mut u32, out: *mut T) -> R {
    if out.is_null() {
        *count = items.len() as u32;
        return OK;
    }
    let n = (*count as usize).min(items.len());
    std::ptr::copy_nonoverlapping(items.as_ptr(), out, n);
    *count = n as u32;
    if n < items.len() {
        R::INCOMPLETE
    } else {
        OK
    }
}

fn ext(name: &CStr) -> vk::ExtensionProperties {
    let mut e = vk::ExtensionProperties::default();
    for (i, b) in name.to_bytes().iter().enumerate() {
        e.extension_name[i] = *b as c_char;
    }
    e.spec_version = 1;
    e
}

/// Walks a pNext chain.
pub(crate) unsafe fn walk(mut p: *mut c_void, mut f: impl FnMut(vk::StructureType, *mut c_void)) {
    while !p.is_null() {
        let b = p as *mut vk::BaseOutStructure<'_>;
        f((*b).s_type, p);
        p = (*b).p_next as *mut c_void;
    }
}

// ---------- instance ----------

unsafe extern "system" fn create_instance(_ci: *const vk::InstanceCreateInfo<'_>, _a: *const c_void, out: *mut vk::Instance) -> R {
    if let Err(e) = ring::start() {
        crate::log::red(&format!("web: {e}"));
        return R::ERROR_INITIALIZATION_FAILED;
    }
    let st = ring::hdr(ring::H_FEATURES).load(Ordering::Relaxed);
    crate::vklog!(
        "web device: WebGPU in the page's GPU worker (features {st:#x}, {} bind groups, {} storage / {} uniform buffers and {} textures a stage)",
        ring::hdr(ring::H_MAX_BIND_GROUPS).load(Ordering::Relaxed),
        ring::hdr(ring::H_MAX_STORAGE_PER_STAGE).load(Ordering::Relaxed),
        ring::hdr(ring::H_MAX_UNIFORM_PER_STAGE).load(Ordering::Relaxed),
        ring::hdr(ring::H_MAX_TEXTURES_PER_STAGE).load(Ordering::Relaxed)
    );
    submit::init();
    *out = vk::Instance::from_raw(&INSTANCE as *const u8 as u64);
    OK
}
unsafe extern "system" fn destroy_instance(_i: vk::Instance, _a: *const c_void) {}
unsafe extern "system" fn enum_instance_exts(_l: *const c_char, count: *mut u32, out: *mut vk::ExtensionProperties) -> R {
    answer(&[ext(ash::khr::surface::NAME), ext(ash::khr::get_surface_capabilities2::NAME)], count, out)
}
unsafe extern "system" fn enum_instance_layers(count: *mut u32, _out: *mut vk::LayerProperties) -> R {
    *count = 0;
    OK
}
unsafe extern "system" fn enum_instance_version(v: *mut u32) -> R {
    *v = vk::API_VERSION_1_3;
    OK
}
unsafe extern "system" fn enum_pdevs(_i: vk::Instance, count: *mut u32, out: *mut vk::PhysicalDevice) -> R {
    answer(&[vk::PhysicalDevice::from_raw(&PDEV as *const u8 as u64)], count, out)
}

fn limit(i: usize, default: u32) -> u32 {
    match ring::hdr(i).load(Ordering::Relaxed) {
        0 => default,
        v => v,
    }
}

fn props() -> vk::PhysicalDeviceProperties {
    let mut p = vk::PhysicalDeviceProperties {
        api_version: vk::API_VERSION_1_3,
        driver_version: 1,
        vendor_id: 0x1ae0,
        device_id: 1,
        device_type: vk::PhysicalDeviceType::DISCRETE_GPU,
        ..Default::default()
    };
    for (i, b) in b"WebGPU (page GPU worker)".iter().enumerate() {
        p.device_name[i] = *b as c_char;
    }
    let tex2d = limit(ring::H_MAX_TEX_2D, 8192);
    let l = &mut p.limits;
    l.max_image_dimension1_d = tex2d;
    l.max_image_dimension2_d = tex2d;
    l.max_image_dimension3_d = 2048;
    l.max_image_dimension_cube = tex2d;
    l.max_image_array_layers = 256;
    l.max_texel_buffer_elements = 1 << 27;
    l.max_uniform_buffer_range = 65536;
    l.max_storage_buffer_range = limit(ring::H_MAX_STORAGE_MB, 128) << 20;
    l.max_push_constants_size = 128;
    l.max_memory_allocation_count = 1 << 20;
    l.max_sampler_allocation_count = 1 << 20;
    l.buffer_image_granularity = 1;
    l.max_bound_descriptor_sets = 8;
    for v in [
        &mut l.max_per_stage_descriptor_samplers,
        &mut l.max_per_stage_descriptor_uniform_buffers,
        &mut l.max_per_stage_descriptor_storage_buffers,
        &mut l.max_per_stage_descriptor_sampled_images,
        &mut l.max_per_stage_descriptor_storage_images,
        &mut l.max_per_stage_descriptor_input_attachments,
        &mut l.max_per_stage_resources,
        &mut l.max_descriptor_set_samplers,
        &mut l.max_descriptor_set_uniform_buffers,
        &mut l.max_descriptor_set_uniform_buffers_dynamic,
        &mut l.max_descriptor_set_storage_buffers,
        &mut l.max_descriptor_set_storage_buffers_dynamic,
        &mut l.max_descriptor_set_sampled_images,
        &mut l.max_descriptor_set_storage_images,
        &mut l.max_descriptor_set_input_attachments,
    ] {
        *v = 1 << 20;
    }
    l.max_vertex_input_attributes = 16;
    l.max_vertex_input_bindings = 8;
    l.max_vertex_input_attribute_offset = 2047;
    l.max_vertex_input_binding_stride = 2048;
    l.max_vertex_output_components = 60;
    l.max_fragment_input_components = 60;
    l.max_fragment_output_attachments = 8;
    l.max_fragment_combined_output_resources = 1 << 20;
    l.max_compute_shared_memory_size = 16384;
    l.max_compute_work_group_count = [65535; 3];
    l.max_compute_work_group_invocations = 256;
    l.max_compute_work_group_size = [256, 256, 64];
    l.sub_pixel_precision_bits = 8;
    l.sub_texel_precision_bits = 8;
    l.mipmap_precision_bits = 8;
    l.max_draw_indexed_index_value = u32::MAX;
    l.max_draw_indirect_count = 1;
    l.max_sampler_lod_bias = 16.0;
    l.max_sampler_anisotropy = 16.0;
    l.max_viewports = 1;
    l.max_viewport_dimensions = [tex2d, tex2d];
    l.viewport_bounds_range = [-32768.0, 32767.0];
    l.min_memory_map_alignment = 64;
    l.min_texel_buffer_offset_alignment = 256;
    l.min_uniform_buffer_offset_alignment = 256;
    l.min_storage_buffer_offset_alignment = 256;
    l.max_texel_offset = 7;
    l.min_texel_offset = -8;
    l.max_texel_gather_offset = 7;
    l.min_texel_gather_offset = -8;
    l.max_framebuffer_width = tex2d;
    l.max_framebuffer_height = tex2d;
    l.max_framebuffer_layers = 1;
    let s14 = vk::SampleCountFlags::TYPE_1 | vk::SampleCountFlags::TYPE_4;
    l.framebuffer_color_sample_counts = s14;
    l.framebuffer_depth_sample_counts = s14;
    l.framebuffer_stencil_sample_counts = s14;
    l.framebuffer_no_attachments_sample_counts = s14;
    l.max_color_attachments = 8;
    l.sampled_image_color_sample_counts = s14;
    l.sampled_image_integer_sample_counts = vk::SampleCountFlags::TYPE_1;
    l.sampled_image_depth_sample_counts = s14;
    l.sampled_image_stencil_sample_counts = vk::SampleCountFlags::TYPE_1;
    l.storage_image_sample_counts = vk::SampleCountFlags::TYPE_1;
    l.max_sample_mask_words = 1;
    l.timestamp_compute_and_graphics = vk::FALSE;
    l.timestamp_period = 1.0;
    l.max_clip_distances = 0;
    l.max_cull_distances = 0;
    l.max_combined_clip_and_cull_distances = 0;
    l.discrete_queue_priorities = 2;
    l.point_size_range = [1.0, 1.0];
    l.line_width_range = [1.0, 1.0];
    l.optimal_buffer_copy_offset_alignment = 256;
    l.optimal_buffer_copy_row_pitch_alignment = 256;
    l.non_coherent_atom_size = 64;
    p
}

unsafe extern "system" fn get_props(_p: vk::PhysicalDevice, out: *mut vk::PhysicalDeviceProperties) {
    *out = props();
}
unsafe extern "system" fn get_props2(_p: vk::PhysicalDevice, out: *mut vk::PhysicalDeviceProperties2<'_>) {
    (*out).properties = props();
    walk((*out).p_next, |t, p| match t {
        vk::StructureType::PHYSICAL_DEVICE_MAINTENANCE_4_PROPERTIES => {
            (*(p as *mut vk::PhysicalDeviceMaintenance4Properties<'_>)).max_buffer_size =
                (limit(ring::H_MAX_STORAGE_MB, 256) as u64) << 20;
        }
        vk::StructureType::PHYSICAL_DEVICE_SUBGROUP_PROPERTIES => {
            (*(p as *mut vk::PhysicalDeviceSubgroupProperties<'_>)).subgroup_size = 32;
        }
        // descriptor counts: the web device binds what each pipeline's shaders use (bind groups made at the draw),
        // so the Vulkan-side counts are not WebGPU's limits
        vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_2_PROPERTIES => {
            let v = &mut *(p as *mut vk::PhysicalDeviceVulkan12Properties<'_>);
            let m = 1 << 20;
            v.max_update_after_bind_descriptors_in_all_pools = m;
            v.max_per_stage_descriptor_update_after_bind_samplers = m;
            v.max_per_stage_descriptor_update_after_bind_uniform_buffers = m;
            v.max_per_stage_descriptor_update_after_bind_storage_buffers = m;
            v.max_per_stage_descriptor_update_after_bind_sampled_images = m;
            v.max_per_stage_descriptor_update_after_bind_storage_images = m;
            v.max_per_stage_descriptor_update_after_bind_input_attachments = m;
            v.max_per_stage_update_after_bind_resources = m;
            v.max_descriptor_set_update_after_bind_samplers = m;
            v.max_descriptor_set_update_after_bind_uniform_buffers = m;
            v.max_descriptor_set_update_after_bind_uniform_buffers_dynamic = m;
            v.max_descriptor_set_update_after_bind_storage_buffers = m;
            v.max_descriptor_set_update_after_bind_storage_buffers_dynamic = m;
            v.max_descriptor_set_update_after_bind_sampled_images = m;
            v.max_descriptor_set_update_after_bind_storage_images = m;
            v.max_descriptor_set_update_after_bind_input_attachments = m;
            v.max_timeline_semaphore_value_difference = u64::MAX;
        }
        vk::StructureType::PHYSICAL_DEVICE_DESCRIPTOR_INDEXING_PROPERTIES => {
            let v = &mut *(p as *mut vk::PhysicalDeviceDescriptorIndexingProperties<'_>);
            let m = 1 << 20;
            v.max_update_after_bind_descriptors_in_all_pools = m;
            v.max_per_stage_descriptor_update_after_bind_samplers = m;
            v.max_per_stage_descriptor_update_after_bind_uniform_buffers = m;
            v.max_per_stage_descriptor_update_after_bind_storage_buffers = m;
            v.max_per_stage_descriptor_update_after_bind_sampled_images = m;
            v.max_per_stage_descriptor_update_after_bind_storage_images = m;
            v.max_per_stage_descriptor_update_after_bind_input_attachments = m;
            v.max_per_stage_update_after_bind_resources = m;
            v.max_descriptor_set_update_after_bind_samplers = m;
            v.max_descriptor_set_update_after_bind_uniform_buffers = m;
            v.max_descriptor_set_update_after_bind_uniform_buffers_dynamic = m;
            v.max_descriptor_set_update_after_bind_storage_buffers = m;
            v.max_descriptor_set_update_after_bind_storage_buffers_dynamic = m;
            v.max_descriptor_set_update_after_bind_sampled_images = m;
            v.max_descriptor_set_update_after_bind_storage_images = m;
            v.max_descriptor_set_update_after_bind_input_attachments = m;
        }
        vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_1_PROPERTIES => {
            let v = &mut *(p as *mut vk::PhysicalDeviceVulkan11Properties<'_>);
            v.subgroup_size = 32;
            v.max_memory_allocation_size = 1 << 32;
        }
        _ => {}
    });
}
unsafe extern "system" fn get_mem_props(_p: vk::PhysicalDevice, out: *mut vk::PhysicalDeviceMemoryProperties) {
    let mut m = vk::PhysicalDeviceMemoryProperties { memory_type_count: 2, memory_heap_count: 2, ..Default::default() };
    m.memory_types[0] = vk::MemoryType { property_flags: vk::MemoryPropertyFlags::DEVICE_LOCAL, heap_index: 0 };
    m.memory_types[1] = vk::MemoryType {
        property_flags: vk::MemoryPropertyFlags::HOST_VISIBLE
            | vk::MemoryPropertyFlags::HOST_COHERENT
            | vk::MemoryPropertyFlags::HOST_CACHED,
        heap_index: 1,
    };
    m.memory_heaps[0] = vk::MemoryHeap { size: 8 << 30, flags: vk::MemoryHeapFlags::DEVICE_LOCAL };
    m.memory_heaps[1] = vk::MemoryHeap { size: 4 << 30, flags: vk::MemoryHeapFlags::empty() };
    *out = m;
}
unsafe extern "system" fn get_mem_props2(p: vk::PhysicalDevice, out: *mut vk::PhysicalDeviceMemoryProperties2<'_>) {
    get_mem_props(p, &mut (*out).memory_properties);
}
// (wasm checks an indirect call's signature exactly: each entry here has its Vulkan signature, void ones too)
unsafe extern "system" fn get_qfams(_p: vk::PhysicalDevice, count: *mut u32, out: *mut vk::QueueFamilyProperties) {
    let q = vk::QueueFamilyProperties {
        queue_flags: vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
        queue_count: 1,
        timestamp_valid_bits: 0,
        min_image_transfer_granularity: vk::Extent3D { width: 1, height: 1, depth: 1 },
    };
    let _ = answer(&[q], count, out);
}
unsafe extern "system" fn get_qfams2(p: vk::PhysicalDevice, count: *mut u32, out: *mut vk::QueueFamilyProperties2<'_>) {
    if out.is_null() {
        get_qfams(p, count, std::ptr::null_mut());
    } else if *count > 0 {
        get_qfams(p, &mut 1, &mut (*out).queue_family_properties);
        *count = 1;
    }
}

fn base_features() -> vk::PhysicalDeviceFeatures {
    let f = ring::hdr(ring::H_FEATURES).load(Ordering::Relaxed);
    vk::PhysicalDeviceFeatures {
        independent_blend: 1,
        depth_clamp: 1,
        depth_bias_clamp: 1,
        sampler_anisotropy: 1,
        texture_compression_bc: (f & ring::F_BC != 0) as u32,
        image_cube_array: 1,
        fragment_stores_and_atomics: 1,
        shader_image_gather_extended: 1,
        shader_storage_image_extended_formats: 1,
        dual_src_blend: 0,
        ..Default::default()
    }
}
unsafe extern "system" fn get_features(_p: vk::PhysicalDevice, out: *mut vk::PhysicalDeviceFeatures) {
    *out = base_features();
}
unsafe extern "system" fn get_features2(_p: vk::PhysicalDevice, out: *mut vk::PhysicalDeviceFeatures2<'_>) {
    (*out).features = base_features();
    walk((*out).p_next, |t, p| match t {
        vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_1_FEATURES => {
            (*(p as *mut vk::PhysicalDeviceVulkan11Features<'_>)).shader_draw_parameters = 1;
        }
        vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_2_FEATURES => {
            let v = &mut *(p as *mut vk::PhysicalDeviceVulkan12Features<'_>);
            v.timeline_semaphore = 1;
            v.descriptor_indexing = 1;
            v.descriptor_binding_partially_bound = 1;
            v.descriptor_binding_sampled_image_update_after_bind = 1;
            v.descriptor_binding_storage_image_update_after_bind = 1;
            v.descriptor_binding_update_unused_while_pending = 1;
            v.runtime_descriptor_array = 1;
            v.scalar_block_layout = 1;
        }
        vk::StructureType::PHYSICAL_DEVICE_VULKAN_1_3_FEATURES => {
            let v = &mut *(p as *mut vk::PhysicalDeviceVulkan13Features<'_>);
            v.dynamic_rendering = 1;
            v.synchronization2 = 1;
            v.maintenance4 = 1;
            // naga reads no SPV_EXT_demote_to_helper_invocation: the replay discards with OpKill instead
            v.shader_demote_to_helper_invocation = 0;
            v.shader_zero_initialize_workgroup_memory = 1;
        }
        vk::StructureType::PHYSICAL_DEVICE_EXTENDED_DYNAMIC_STATE_2_FEATURES_EXT => {
            (*(p as *mut vk::PhysicalDeviceExtendedDynamicState2FeaturesEXT<'_>)).extended_dynamic_state2 = 1;
        }
        vk::StructureType::PHYSICAL_DEVICE_EXTENDED_DYNAMIC_STATE_3_FEATURES_EXT => {
            let v = &mut *(p as *mut vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT<'_>);
            v.extended_dynamic_state3_color_blend_enable = 1;
            v.extended_dynamic_state3_color_blend_equation = 1;
            v.extended_dynamic_state3_color_write_mask = 1;
            v.extended_dynamic_state3_depth_clip_negative_one_to_one = 1;
            v.extended_dynamic_state3_depth_clamp_enable = 1;
            v.extended_dynamic_state3_rasterization_samples = 1;
            v.extended_dynamic_state3_alpha_to_coverage_enable = 1;
        }
        vk::StructureType::PHYSICAL_DEVICE_VERTEX_INPUT_DYNAMIC_STATE_FEATURES_EXT => {
            (*(p as *mut vk::PhysicalDeviceVertexInputDynamicStateFeaturesEXT<'_>)).vertex_input_dynamic_state = 1;
        }
        vk::StructureType::PHYSICAL_DEVICE_DEPTH_CLIP_CONTROL_FEATURES_EXT => {
            (*(p as *mut vk::PhysicalDeviceDepthClipControlFeaturesEXT<'_>)).depth_clip_control = 1;
        }
        _ => {}
    });
}
unsafe extern "system" fn enum_dev_exts(_p: vk::PhysicalDevice, _l: *const c_char, count: *mut u32, out: *mut vk::ExtensionProperties) -> R {
    answer(
        &[
            ext(ash::khr::swapchain::NAME),
            ext(ash::ext::extended_dynamic_state2::NAME),
            ext(ash::ext::extended_dynamic_state3::NAME),
            ext(ash::ext::vertex_input_dynamic_state::NAME),
            ext(ash::ext::depth_clip_control::NAME),
        ],
        count,
        out,
    )
}
unsafe extern "system" fn get_format_props(_p: vk::PhysicalDevice, f: vk::Format, out: *mut vk::FormatProperties) {
    *out = format_props(f);
}
fn format_props(f: vk::Format) -> vk::FormatProperties {
    let feats = ring::hdr(ring::H_FEATURES).load(Ordering::Relaxed);
    let t = fmt::tex(f);
    if t.is_some_and(|t| t.name.starts_with("bc") && feats & ring::F_BC == 0)
        || (f == vk::Format::D32_SFLOAT_S8_UINT && feats & ring::F_D32S8 == 0)
    {
        return vk::FormatProperties::default();
    }
    fmt::features(f)
}
unsafe extern "system" fn get_format_props2(_p: vk::PhysicalDevice, f: vk::Format, out: *mut vk::FormatProperties2<'_>) {
    (*out).format_properties = format_props(f);
}
unsafe extern "system" fn get_image_format_props(
    _p: vk::PhysicalDevice,
    f: vk::Format,
    ty: vk::ImageType,
    _t: vk::ImageTiling,
    usage: vk::ImageUsageFlags,
    _fl: vk::ImageCreateFlags,
    out: *mut vk::ImageFormatProperties,
) -> R {
    let Some(t) = fmt::tex(f) else { return R::ERROR_FORMAT_NOT_SUPPORTED };
    if format_props(f).optimal_tiling_features.is_empty()
        || (usage.contains(vk::ImageUsageFlags::STORAGE) && !t.storage)
        || (usage.intersects(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            && !t.render)
        || (ty == vk::ImageType::TYPE_3D && (t.depth || t.stencil))
    {
        return R::ERROR_FORMAT_NOT_SUPPORTED;
    }
    let d = limit(ring::H_MAX_TEX_2D, 8192);
    *out = vk::ImageFormatProperties {
        max_extent: vk::Extent3D { width: d, height: d, depth: if ty == vk::ImageType::TYPE_3D { 2048 } else { 1 } },
        max_mip_levels: 15,
        max_array_layers: 256,
        sample_counts: vk::SampleCountFlags::TYPE_1 | vk::SampleCountFlags::TYPE_4,
        max_resource_size: 1 << 32,
    };
    OK
}
unsafe extern "system" fn get_sparse_props(
    _p: vk::PhysicalDevice,
    _f: vk::Format,
    _t: vk::ImageType,
    _s: vk::SampleCountFlags,
    _u: vk::ImageUsageFlags,
    _ti: vk::ImageTiling,
    count: *mut u32,
    _out: *mut c_void,
) {
    *count = 0;
}

// ---------- surface (the page canvas) ----------

fn canvas() -> vk::Extent2D {
    vk::Extent2D { width: limit(ring::H_CANVAS_W, 1280), height: limit(ring::H_CANVAS_H, 720) }
}
unsafe extern "system" fn surface_support(_p: vk::PhysicalDevice, _q: u32, _s: vk::SurfaceKHR, out: *mut vk::Bool32) -> R {
    *out = 1;
    OK
}
unsafe extern "system" fn surface_caps(_p: vk::PhysicalDevice, _s: vk::SurfaceKHR, out: *mut vk::SurfaceCapabilitiesKHR) -> R {
    let e = canvas();
    *out = vk::SurfaceCapabilitiesKHR {
        min_image_count: 2,
        max_image_count: 3,
        current_extent: e,
        min_image_extent: vk::Extent2D { width: 1, height: 1 },
        max_image_extent: vk::Extent2D { width: 8192, height: 8192 },
        max_image_array_layers: 1,
        supported_transforms: vk::SurfaceTransformFlagsKHR::IDENTITY,
        current_transform: vk::SurfaceTransformFlagsKHR::IDENTITY,
        supported_composite_alpha: vk::CompositeAlphaFlagsKHR::OPAQUE,
        supported_usage_flags: vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::TRANSFER_SRC,
    };
    OK
}
unsafe extern "system" fn surface_formats(_p: vk::PhysicalDevice, _s: vk::SurfaceKHR, count: *mut u32, out: *mut vk::SurfaceFormatKHR) -> R {
    // WEB-W8: the sRGB format too (as a native surface offers it): an sRGB picture blitted into it is encoded again,
    // where into the UNORM one it arrived decoded (too dark). The canvas stays bgra8unorm; the present's copy between
    // the two differs only in sRGB-ness, which WebGPU copies byte for byte.
    answer(
        &[
            vk::SurfaceFormatKHR { format: vk::Format::B8G8R8A8_UNORM, color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR },
            vk::SurfaceFormatKHR { format: vk::Format::B8G8R8A8_SRGB, color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR },
        ],
        count,
        out,
    )
}

/// The swap chain image formats the canvas surface offers (`surface_formats`); anything else is made as UNORM.
fn swapchain_format(f: vk::Format) -> vk::Format {
    match f {
        vk::Format::B8G8R8A8_SRGB => f,
        _ => vk::Format::B8G8R8A8_UNORM,
    }
}
unsafe extern "system" fn surface_modes(_p: vk::PhysicalDevice, _s: vk::SurfaceKHR, count: *mut u32, out: *mut vk::PresentModeKHR) -> R {
    answer(&[vk::PresentModeKHR::FIFO], count, out)
}
unsafe extern "system" fn destroy_surface(_i: vk::Instance, _s: vk::SurfaceKHR, _a: *const c_void) {}

unsafe extern "system" fn create_swapchain(_d: vk::Device, ci: *const vk::SwapchainCreateInfoKHR<'_>, _a: *const c_void, out: *mut vk::SwapchainKHR) -> R {
    let ci = &*ci;
    let n = ci.min_image_count.clamp(2, 3);
    let format = swapchain_format(ci.image_format);
    let mut images = Vec::new();
    for _ in 0..n {
        let mut im = Image {
            ty: vk::ImageType::TYPE_2D,
            format,
            extent: vk::Extent3D { width: ci.image_extent.width, height: ci.image_extent.height, depth: 1 },
            mips: 1,
            layers: 1,
            samples: 1,
            usage: ci.image_usage | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::empty(),
            id: 0,
            tex: fmt::tex(format),
        };
        im.id = obj::gpu_texture(&im, true);
        let h = obj::new_handle();
        objs().img.insert(h, im);
        images.push(h);
    }
    let h = obj::new_handle();
    objs().swapchain.insert(h, obj::Swapchain { images, extent: ci.image_extent, next: 0 });
    *out = vk::SwapchainKHR::from_raw(h);
    OK
}
unsafe extern "system" fn destroy_swapchain(_d: vk::Device, s: vk::SwapchainKHR, _a: *const c_void) {
    let mut o = objs();
    if let Some(sc) = o.swapchain.remove(&s.as_raw()) {
        for i in sc.images {
            if let Some(im) = o.img.remove(&i) {
                obj::destroy(im.id);
            }
        }
    }
}
unsafe extern "system" fn swapchain_images(_d: vk::Device, s: vk::SwapchainKHR, count: *mut u32, out: *mut vk::Image) -> R {
    let o = objs();
    let Some(sc) = o.swapchain.get(&s.as_raw()) else { return R::ERROR_SURFACE_LOST_KHR };
    let v: Vec<vk::Image> = sc.images.iter().map(|&h| vk::Image::from_raw(h)).collect();
    answer(&v, count, out)
}
unsafe extern "system" fn acquire_next_image(
    _d: vk::Device,
    s: vk::SwapchainKHR,
    _t: u64,
    _sem: vk::Semaphore,
    fence: vk::Fence,
    index: *mut u32,
) -> R {
    let mut o = objs();
    let Some(sc) = o.swapchain.get_mut(&s.as_raw()) else { return R::ERROR_SURFACE_LOST_KHR };
    *index = sc.next;
    let stale = sc.extent != canvas();
    sc.next = (sc.next + 1) % sc.images.len() as u32;
    if let Some(f) = o.sem.get(&fence.as_raw()) {
        f.word.0.store(1, Ordering::Release);
    }
    if stale {
        R::SUBOPTIMAL_KHR
    } else {
        OK
    }
}
unsafe extern "system" fn queue_present(_q: vk::Queue, pi: *const vk::PresentInfoKHR<'_>) -> R {
    let pi = &*pi;
    let scs = sl(pi.p_swapchains, pi.swapchain_count);
    let idx = sl(pi.p_image_indices, pi.swapchain_count);
    for (s, &i) in scs.iter().zip(idx) {
        let id = {
            let o = objs();
            let Some(sc) = o.swapchain.get(&s.as_raw()) else { continue };
            o.img.get(&sc.images[i as usize]).map_or(0, |im| im.id)
        };
        ring::push(op::PRESENT, &id.to_le_bytes());
    }
    OK
}

// ---------- device ----------

unsafe extern "system" fn create_device(_p: vk::PhysicalDevice, _ci: *const vk::DeviceCreateInfo<'_>, _a: *const c_void, out: *mut vk::Device) -> R {
    *out = device_handle();
    OK
}
unsafe extern "system" fn destroy_device(_d: vk::Device, _a: *const c_void) {}
unsafe extern "system" fn get_queue(_d: vk::Device, _f: u32, _i: u32, out: *mut vk::Queue) {
    *out = vk::Queue::from_raw(&QUEUE as *const u8 as u64);
}
unsafe extern "system" fn get_queue2(_d: vk::Device, _i: *const c_void, out: *mut vk::Queue) {
    *out = vk::Queue::from_raw(&QUEUE as *const u8 as u64);
}
unsafe extern "system" fn device_wait_idle(_d: vk::Device) -> R {
    submit::wait_idle();
    OK
}
unsafe extern "system" fn queue_wait_idle(_q: vk::Queue) -> R {
    submit::wait_idle();
    OK
}

// ---------- memory ----------

unsafe extern "system" fn allocate_memory(_d: vk::Device, ai: *const vk::MemoryAllocateInfo<'_>, _a: *const c_void, out: *mut vk::DeviceMemory) -> R {
    let ai = &*ai;
    let host = if ai.memory_type_index == 1 {
        let l = std::alloc::Layout::from_size_align(ai.allocation_size.max(1) as usize, 256).unwrap();
        let p = std::alloc::alloc_zeroed(l);
        if p.is_null() {
            return R::ERROR_OUT_OF_HOST_MEMORY;
        }
        p
    } else {
        std::ptr::null_mut()
    };
    let h = obj::new_handle();
    objs().mem.insert(h, Memory { size: ai.allocation_size, host });
    *out = vk::DeviceMemory::from_raw(h);
    OK
}
unsafe extern "system" fn free_memory(_d: vk::Device, m: vk::DeviceMemory, _a: *const c_void) {
    if let Some(m) = objs().mem.remove(&m.as_raw()) {
        if !m.host.is_null() {
            std::alloc::dealloc(m.host, std::alloc::Layout::from_size_align(m.size.max(1) as usize, 256).unwrap());
        }
    }
}
unsafe extern "system" fn map_memory(_d: vk::Device, m: vk::DeviceMemory, off: u64, _s: u64, _f: vk::MemoryMapFlags, out: *mut *mut c_void) -> R {
    let o = objs();
    match o.mem.get(&m.as_raw()) {
        Some(m) if !m.host.is_null() => {
            *out = m.host.add(off as usize) as *mut c_void;
            OK
        }
        _ => R::ERROR_MEMORY_MAP_FAILED,
    }
}
unsafe extern "system" fn unmap_memory(_d: vk::Device, _m: vk::DeviceMemory) {}
unsafe extern "system" fn flush_ranges(_d: vk::Device, _n: u32, _r: *const c_void) -> R {
    OK
}

// ---------- buffers ----------

unsafe extern "system" fn create_buffer(_d: vk::Device, ci: *const vk::BufferCreateInfo<'_>, _a: *const c_void, out: *mut vk::Buffer) -> R {
    let ci = &*ci;
    let id = obj::gpu_buffer(ci.size, ci.usage);
    let h = obj::new_handle();
    objs().buf.insert(h, obj::Buffer { size: ci.size, usage: ci.usage, id, host: std::ptr::null_mut() });
    *out = vk::Buffer::from_raw(h);
    OK
}
unsafe extern "system" fn destroy_buffer(_d: vk::Device, b: vk::Buffer, _a: *const c_void) {
    if let Some(b) = objs().buf.remove(&b.as_raw()) {
        submit::release(b.id);
    }
}
fn buffer_reqs(size: u64) -> vk::MemoryRequirements {
    vk::MemoryRequirements { size: size.next_multiple_of(256).max(256), alignment: 256, memory_type_bits: 0b11 }
}
unsafe extern "system" fn buffer_mem_reqs(_d: vk::Device, b: vk::Buffer, out: *mut vk::MemoryRequirements) {
    let size = objs().buf.get(&b.as_raw()).map_or(256, |b| b.size);
    *out = buffer_reqs(size);
}
unsafe extern "system" fn buffer_mem_reqs2(_d: vk::Device, i: *const vk::BufferMemoryRequirementsInfo2<'_>, out: *mut vk::MemoryRequirements2<'_>) {
    let size = objs().buf.get(&(*i).buffer.as_raw()).map_or(256, |b| b.size);
    (*out).memory_requirements = buffer_reqs(size);
}
unsafe extern "system" fn device_buffer_mem_reqs(_d: vk::Device, i: *const vk::DeviceBufferMemoryRequirements<'_>, out: *mut vk::MemoryRequirements2<'_>) {
    (*out).memory_requirements = buffer_reqs((*(*i).p_create_info).size);
}
fn bind_buffer(b: u64, m: u64, off: u64) {
    let mut o = objs();
    let host = o.mem.get(&m).map_or(std::ptr::null_mut(), |m| m.host);
    if let Some(b) = o.buf.get_mut(&b) {
        // SAFETY: inside the allocation (the allocator placed the buffer there)
        b.host = if host.is_null() { host } else { unsafe { host.add(off as usize) } };
    }
}
unsafe extern "system" fn bind_buffer_memory(_d: vk::Device, b: vk::Buffer, m: vk::DeviceMemory, off: u64) -> R {
    bind_buffer(b.as_raw(), m.as_raw(), off);
    OK
}
unsafe extern "system" fn bind_buffer_memory2(_d: vk::Device, n: u32, i: *const vk::BindBufferMemoryInfo<'_>) -> R {
    for b in sl(i, n) {
        bind_buffer(b.buffer.as_raw(), b.memory.as_raw(), b.memory_offset);
    }
    OK
}
unsafe extern "system" fn create_buffer_view(_d: vk::Device, _ci: *const c_void, _a: *const c_void, out: *mut vk::BufferView) -> R {
    *out = vk::BufferView::from_raw(obj::new_handle());
    OK
}
unsafe extern "system" fn destroy_nothing(_d: vk::Device, _h: u64, _a: *const c_void) {}

// ---------- images ----------

unsafe extern "system" fn create_image(_d: vk::Device, ci: *const vk::ImageCreateInfo<'_>, _a: *const c_void, out: *mut vk::Image) -> R {
    let ci = &*ci;
    let tex = fmt::tex(ci.format);
    if tex.is_none() {
        return R::ERROR_FORMAT_NOT_SUPPORTED;
    }
    let mut im = Image {
        ty: ci.image_type,
        format: ci.format,
        extent: ci.extent,
        mips: ci.mip_levels,
        layers: ci.array_layers,
        samples: ci.samples.as_raw().max(1),
        usage: ci.usage,
        flags: ci.flags,
        id: 0,
        tex,
    };
    im.id = obj::gpu_texture(&im, false);
    let h = obj::new_handle();
    objs().img.insert(h, im);
    *out = vk::Image::from_raw(h);
    OK
}
unsafe extern "system" fn destroy_image(_d: vk::Device, i: vk::Image, _a: *const c_void) {
    if let Some(im) = objs().img.remove(&i.as_raw()) {
        submit::release(im.id);
    }
}
fn image_reqs(im: Option<&Image>) -> vk::MemoryRequirements {
    let size = im.map_or(256, |im| {
        let t = im.tex.unwrap();
        let blocks = (im.extent.width.div_ceil(t.bw) as u64) * (im.extent.height.div_ceil(t.bh) as u64);
        blocks * t.bytes as u64 * im.extent.depth.max(1) as u64 * im.layers.max(1) as u64 * im.samples as u64 * 4 / 3 + 256
    });
    vk::MemoryRequirements { size: size.next_multiple_of(256), alignment: 256, memory_type_bits: 0b01 }
}
unsafe extern "system" fn image_mem_reqs(_d: vk::Device, i: vk::Image, out: *mut vk::MemoryRequirements) {
    *out = image_reqs(objs().img.get(&i.as_raw()));
}
unsafe extern "system" fn image_mem_reqs2(_d: vk::Device, i: *const vk::ImageMemoryRequirementsInfo2<'_>, out: *mut vk::MemoryRequirements2<'_>) {
    (*out).memory_requirements = image_reqs(objs().img.get(&(*i).image.as_raw()));
}
unsafe extern "system" fn bind_image_memory(_d: vk::Device, _i: vk::Image, _m: vk::DeviceMemory, _o: u64) -> R {
    OK
}
unsafe extern "system" fn bind_image_memory2(_d: vk::Device, _n: u32, _i: *const c_void) -> R {
    OK
}
unsafe extern "system" fn subresource_layout(_d: vk::Device, _i: vk::Image, _s: *const c_void, out: *mut vk::SubresourceLayout) {
    *out = vk::SubresourceLayout::default();
}

fn swizzle_str(c: vk::ComponentMapping) -> String {
    let one = |s: vk::ComponentSwizzle, ident: char| match s {
        vk::ComponentSwizzle::IDENTITY => ident,
        vk::ComponentSwizzle::R => 'r',
        vk::ComponentSwizzle::G => 'g',
        vk::ComponentSwizzle::B => 'b',
        vk::ComponentSwizzle::A => 'a',
        vk::ComponentSwizzle::ZERO => '0',
        _ => '1',
    };
    [one(c.r, 'r'), one(c.g, 'g'), one(c.b, 'b'), one(c.a, 'a')].iter().collect()
}

unsafe extern "system" fn create_image_view(_d: vk::Device, ci: *const vk::ImageViewCreateInfo<'_>, _a: *const c_void, out: *mut vk::ImageView) -> R {
    let ci = &*ci;
    let mut o = objs();
    let Some(im) = o.img.get(&ci.image.as_raw()).cloned() else { return R::ERROR_UNKNOWN };
    let r = ci.subresource_range;
    let mut v = View {
        image: ci.image.as_raw(),
        id: 0,
        ty: ci.view_type,
        format: ci.format,
        base_mip: r.base_mip_level,
        mips: if r.level_count == vk::REMAINING_MIP_LEVELS { im.mips - r.base_mip_level } else { r.level_count },
        base_layer: r.base_array_layer,
        layers: if r.layer_count == vk::REMAINING_ARRAY_LAYERS {
            im.layers - r.base_array_layer
        } else {
            r.layer_count
        },
        aspect: r.aspect_mask,
    };
    if im.samples > 1 {
        v.mips = 1;
    }
    drop(o);
    v.id = obj::gpu_view(&v, &im, &swizzle_str(ci.components));
    let h = obj::new_handle();
    o = objs();
    o.view.insert(h, v);
    *out = vk::ImageView::from_raw(h);
    OK
}
unsafe extern "system" fn destroy_image_view(_d: vk::Device, v: vk::ImageView, _a: *const c_void) {
    if let Some(v) = objs().view.remove(&v.as_raw()) {
        submit::release(v.id);
    }
}
unsafe extern "system" fn create_sampler(_d: vk::Device, ci: *const vk::SamplerCreateInfo<'_>, _a: *const c_void, out: *mut vk::Sampler) -> R {
    let id = obj::gpu_sampler(&*ci);
    let h = obj::new_handle();
    objs().sampler.insert(h, id);
    *out = vk::Sampler::from_raw(h);
    OK
}
unsafe extern "system" fn destroy_sampler(_d: vk::Device, s: vk::Sampler, _a: *const c_void) {
    if let Some(id) = objs().sampler.remove(&s.as_raw()) {
        submit::release(id);
    }
}

// ---------- shaders, layouts, descriptor sets ----------

unsafe extern "system" fn create_shader_module(_d: vk::Device, ci: *const vk::ShaderModuleCreateInfo<'_>, _a: *const c_void, out: *mut vk::ShaderModule) -> R {
    let ci = &*ci;
    let words = std::slice::from_raw_parts(ci.p_code, ci.code_size / 4);
    let conv = shaders::wgsl::convert(words).map(Arc::new);
    let id = match &conv {
        Ok(c) => {
            let id = obj::new_id();
            obj::json(op::SHADER, &serde_json::json!({ "id": id, "code": c.code }));
            id
        }
        Err(e) => {
            crate::log::red(&format!("web: a shader module WebGPU cannot run: {e}"));
            return R::ERROR_INVALID_SHADER_NV;
        }
    };
    let h = obj::new_handle();
    objs().module.insert(h, obj::ShaderModule { conv, id });
    *out = vk::ShaderModule::from_raw(h);
    OK
}
unsafe extern "system" fn destroy_shader_module(_d: vk::Device, m: vk::ShaderModule, _a: *const c_void) {
    // pipelines keep their modules' WGSL by id: the worker's module stays while a pipeline uses it
    objs().module.remove(&m.as_raw());
}
unsafe extern "system" fn create_dsl(_d: vk::Device, ci: *const vk::DescriptorSetLayoutCreateInfo<'_>, _a: *const c_void, out: *mut vk::DescriptorSetLayout) -> R {
    let ci = &*ci;
    let bindings = sl(ci.p_bindings, ci.binding_count)
        .iter()
        .map(|b| (b.binding, (b.descriptor_type, b.descriptor_count)))
        .collect();
    let h = obj::new_handle();
    objs().dsl.insert(h, Dsl { bindings });
    *out = vk::DescriptorSetLayout::from_raw(h);
    OK
}
unsafe extern "system" fn create_playout(_d: vk::Device, ci: *const vk::PipelineLayoutCreateInfo<'_>, _a: *const c_void, out: *mut vk::PipelineLayout) -> R {
    let ci = &*ci;
    let sets = sl(ci.p_set_layouts, ci.set_layout_count).iter().map(|s| s.as_raw()).collect();
    let h = obj::new_handle();
    objs().playout.insert(h, PipelineLayout { sets });
    *out = vk::PipelineLayout::from_raw(h);
    OK
}
unsafe extern "system" fn create_dpool(_d: vk::Device, _ci: *const c_void, _a: *const c_void, out: *mut vk::DescriptorPool) -> R {
    *out = vk::DescriptorPool::from_raw(obj::new_handle());
    OK
}
unsafe extern "system" fn destroy_dpool(_d: vk::Device, p: vk::DescriptorPool, _a: *const c_void) {
    reset_dpool(_d, p, vk::DescriptorPoolResetFlags::empty());
}
unsafe extern "system" fn reset_dpool(_d: vk::Device, p: vk::DescriptorPool, _f: vk::DescriptorPoolResetFlags) -> R {
    objs().dset.retain(|_, s| s.pool != p.as_raw());
    OK
}
unsafe extern "system" fn alloc_dsets(_d: vk::Device, ai: *const vk::DescriptorSetAllocateInfo<'_>, out: *mut vk::DescriptorSet) -> R {
    let ai = &*ai;
    let mut o = objs();
    for i in 0..ai.descriptor_set_count as usize {
        let h = obj::new_handle();
        o.dset.insert(h, obj::DSet { pool: ai.descriptor_pool.as_raw(), d: Default::default() });
        *out.add(i) = vk::DescriptorSet::from_raw(h);
    }
    OK
}
unsafe extern "system" fn free_dsets(_d: vk::Device, _p: vk::DescriptorPool, n: u32, s: *const vk::DescriptorSet) -> R {
    let mut o = objs();
    for s in sl(s, n) {
        o.dset.remove(&s.as_raw());
    }
    OK
}
unsafe extern "system" fn update_dsets(_d: vk::Device, nw: u32, w: *const vk::WriteDescriptorSet<'_>, nc: u32, c: *const vk::CopyDescriptorSet<'_>) {
    let mut o = objs();
    for w in sl(w, nw) {
        let n = w.descriptor_count;
        let descs: Vec<Desc> = match w.descriptor_type {
            vk::DescriptorType::UNIFORM_BUFFER
            | vk::DescriptorType::STORAGE_BUFFER
            | vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC
            | vk::DescriptorType::STORAGE_BUFFER_DYNAMIC => sl(w.p_buffer_info, n)
                .iter()
                .map(|b| {
                    if b.buffer == vk::Buffer::null() {
                        Desc::None
                    } else {
                        Desc::Buf { buf: b.buffer.as_raw(), off: b.offset, range: b.range }
                    }
                })
                .collect(),
            vk::DescriptorType::UNIFORM_TEXEL_BUFFER | vk::DescriptorType::STORAGE_TEXEL_BUFFER => {
                sl(w.p_texel_buffer_view, n).iter().map(|v| Desc::TexelBuf(v.as_raw())).collect()
            }
            vk::DescriptorType::SAMPLER => sl(w.p_image_info, n).iter().map(|i| Desc::Sampler(i.sampler.as_raw())).collect(),
            _ => sl(w.p_image_info, n)
                .iter()
                .map(|i| {
                    if i.image_view == vk::ImageView::null() {
                        Desc::None
                    } else {
                        Desc::Img { view: i.image_view.as_raw(), sampler: i.sampler.as_raw() }
                    }
                })
                .collect(),
        };
        if let Some(s) = o.dset.get_mut(&w.dst_set.as_raw()) {
            for (k, d) in descs.into_iter().enumerate() {
                s.d.insert((w.dst_binding, w.dst_array_element + k as u32), d);
            }
        }
    }
    for c in sl(c, nc) {
        let src: Vec<((u32, u32), Desc)> = o.dset.get(&c.src_set.as_raw()).map_or(Vec::new(), |s| {
            (0..c.descriptor_count)
                .filter_map(|k| {
                    s.d.get(&(c.src_binding, c.src_array_element + k))
                        .map(|d| ((c.dst_binding, c.dst_array_element + k), *d))
                })
                .collect()
        });
        if let Some(s) = o.dset.get_mut(&c.dst_set.as_raw()) {
            s.d.extend(src);
        }
    }
}

// ---------- pipelines ----------

unsafe extern "system" fn create_pcache(_d: vk::Device, _ci: *const c_void, _a: *const c_void, out: *mut vk::PipelineCache) -> R {
    *out = vk::PipelineCache::from_raw(obj::new_handle());
    OK
}
unsafe extern "system" fn pcache_data(_d: vk::Device, _c: vk::PipelineCache, size: *mut usize, _out: *mut c_void) -> R {
    *size = 0;
    OK
}
unsafe extern "system" fn merge_pcaches(_d: vk::Device, _c: vk::PipelineCache, _n: u32, _s: *const c_void) -> R {
    OK
}

unsafe fn gfx_state(ci: &vk::GraphicsPipelineCreateInfo<'_>) -> cmd::Dyn {
    let mut d = cmd::Dyn::default();
    if let Some(ia) = ci.p_input_assembly_state.as_ref() {
        d.topology = ia.topology;
        d.restart = ia.primitive_restart_enable != 0;
    }
    if let Some(rs) = ci.p_rasterization_state.as_ref() {
        d.cull = rs.cull_mode;
        d.front = rs.front_face;
        d.discard = rs.rasterizer_discard_enable != 0;
        d.depth_clamp = rs.depth_clamp_enable != 0;
        d.bias_enable = rs.depth_bias_enable != 0;
        d.bias = [rs.depth_bias_constant_factor, rs.depth_bias_clamp, rs.depth_bias_slope_factor];
        d.polygon = rs.polygon_mode;
    }
    if let Some(ds) = ci.p_depth_stencil_state.as_ref() {
        d.depth_test = ds.depth_test_enable != 0;
        d.depth_write = ds.depth_write_enable != 0;
        d.depth_compare = ds.depth_compare_op;
        d.stencil_test = ds.stencil_test_enable != 0;
        for (i, f) in [ds.front, ds.back].iter().enumerate() {
            d.stencil_ops[i] = (f.fail_op, f.pass_op, f.depth_fail_op, f.compare_op);
            d.stencil_read[i] = f.compare_mask;
            d.stencil_write[i] = f.write_mask;
            d.stencil_ref[i] = f.reference;
        }
    }
    if let Some(ms) = ci.p_multisample_state.as_ref() {
        d.samples = ms.rasterization_samples.as_raw().max(1);
        d.a2c = ms.alpha_to_coverage_enable != 0;
    }
    if let Some(cb) = ci.p_color_blend_state.as_ref() {
        d.logic_op = cb.logic_op_enable != 0;
        d.blend_const = cb.blend_constants;
        for (i, a) in sl(cb.p_attachments, cb.attachment_count).iter().enumerate().take(8) {
            d.blend[i] = a.blend_enable != 0;
            d.eq[i] = vk::ColorBlendEquationEXT {
                src_color_blend_factor: a.src_color_blend_factor,
                dst_color_blend_factor: a.dst_color_blend_factor,
                color_blend_op: a.color_blend_op,
                src_alpha_blend_factor: a.src_alpha_blend_factor,
                dst_alpha_blend_factor: a.dst_alpha_blend_factor,
                alpha_blend_op: a.alpha_blend_op,
            };
            d.mask[i] = a.color_write_mask;
        }
    }
    if let Some(vi) = ci.p_vertex_input_state.as_ref() {
        d.vbind = sl(vi.p_vertex_binding_descriptions, vi.vertex_binding_description_count)
            .iter()
            .map(|b| (b.binding, b.stride, b.input_rate == vk::VertexInputRate::INSTANCE))
            .collect();
        d.vattr = sl(vi.p_vertex_attribute_descriptions, vi.vertex_attribute_description_count)
            .iter()
            .map(|a| (a.location, a.binding, a.format, a.offset))
            .collect();
    }
    if let Some(vp) = ci.p_viewport_state.as_ref() {
        if !vp.p_viewports.is_null() && vp.viewport_count > 0 {
            d.viewport = Some(*vp.p_viewports);
        }
        if !vp.p_scissors.is_null() && vp.scissor_count > 0 {
            d.scissor = Some(*vp.p_scissors);
        }
    }
    d
}

unsafe extern "system" fn create_gfx_pipelines(
    _d: vk::Device,
    _c: vk::PipelineCache,
    n: u32,
    cis: *const vk::GraphicsPipelineCreateInfo<'_>,
    _a: *const c_void,
    out: *mut vk::Pipeline,
) -> R {
    let mut res = OK;
    for (i, ci) in sl(cis, n).iter().enumerate() {
        let (mut vs, mut fs) = (0, 0);
        let mut bad = None;
        for s in sl(ci.p_stages, ci.stage_count) {
            match s.stage {
                vk::ShaderStageFlags::VERTEX => vs = s.module.as_raw(),
                vk::ShaderStageFlags::FRAGMENT => fs = s.module.as_raw(),
                st => bad = Some(st),
            }
        }
        if let Some(st) = bad {
            crate::log::red_once(&format!("web: a pipeline with a {st:?} stage (WebGPU has none): refused"));
            *out.add(i) = vk::Pipeline::null();
            res = R::ERROR_UNKNOWN;
            continue;
        }
        let (mut colors, mut depth, mut stencil) = (Vec::new(), vk::Format::UNDEFINED, vk::Format::UNDEFINED);
        walk(ci.p_next as *mut c_void, |t, p| {
            if t == vk::StructureType::PIPELINE_RENDERING_CREATE_INFO {
                let r = &*(p as *const vk::PipelineRenderingCreateInfo<'_>);
                colors = sl(r.p_color_attachment_formats, r.color_attachment_count).to_vec();
                depth = r.depth_attachment_format;
                stencil = r.stencil_attachment_format;
            }
        });
        let dynamic = ci
            .p_dynamic_state
            .as_ref()
            .map(|d| sl(d.p_dynamic_states, d.dynamic_state_count).to_vec())
            .unwrap_or_default();
        let (vs, fs) = {
            let o = objs();
            let st = |h: u64| o.module.get(&h).and_then(|m| m.conv.as_ref().ok().map(|c| (c.clone(), m.id)));
            (st(vs), if fs == 0 { None } else { st(fs) })
        };
        let Some(vs) = vs else {
            crate::log::red_once("web: a graphics pipeline without a vertex module: refused");
            *out.add(i) = vk::Pipeline::null();
            res = R::ERROR_UNKNOWN;
            continue;
        };
        let g = Gfx { vs, fs, colors, depth, stencil, dynamic, stat: gfx_state(ci) };
        let h = obj::new_handle();
        objs().pipe.insert(h, Arc::new(Pipeline::Gfx(Box::new(g))));
        *out.add(i) = vk::Pipeline::from_raw(h);
    }
    res
}
unsafe extern "system" fn create_compute_pipelines(
    _d: vk::Device,
    _c: vk::PipelineCache,
    n: u32,
    cis: *const vk::ComputePipelineCreateInfo<'_>,
    _a: *const c_void,
    out: *mut vk::Pipeline,
) -> R {
    let mut res = OK;
    for (i, ci) in sl(cis, n).iter().enumerate() {
        let cs = {
            let o = objs();
            o.module.get(&ci.stage.module.as_raw()).and_then(|m| m.conv.as_ref().ok().map(|c| (c.clone(), m.id)))
        };
        let Some(cs) = cs else {
            *out.add(i) = vk::Pipeline::null();
            res = R::ERROR_UNKNOWN;
            continue;
        };
        let h = obj::new_handle();
        objs().pipe.insert(h, Arc::new(Pipeline::Compute { cs }));
        *out.add(i) = vk::Pipeline::from_raw(h);
    }
    res
}
unsafe extern "system" fn destroy_pipeline(_d: vk::Device, p: vk::Pipeline, _a: *const c_void) {
    objs().pipe.remove(&p.as_raw());
    submit::forget_pipeline(p.as_raw());
}

// ---------- command pools and buffers ----------

unsafe extern "system" fn create_cpool(_d: vk::Device, _ci: *const c_void, _a: *const c_void, out: *mut vk::CommandPool) -> R {
    *out = vk::CommandPool::from_raw(obj::new_handle());
    OK
}
unsafe extern "system" fn reset_cpool(_d: vk::Device, _p: vk::CommandPool, _f: u32) -> R {
    OK
}
unsafe extern "system" fn alloc_cbs(_d: vk::Device, ai: *const vk::CommandBufferAllocateInfo<'_>, out: *mut vk::CommandBuffer) -> R {
    for i in 0..(*ai).command_buffer_count as usize {
        *out.add(i) = cmd::new_cb();
    }
    OK
}
unsafe extern "system" fn free_cbs(_d: vk::Device, _p: vk::CommandPool, n: u32, c: *const vk::CommandBuffer) {
    for c in sl(c, n) {
        if *c != vk::CommandBuffer::null() {
            cmd::free_cb(*c);
        }
    }
}

// ---------- sync ----------

unsafe extern "system" fn create_semaphore(_d: vk::Device, ci: *const vk::SemaphoreCreateInfo<'_>, _a: *const c_void, out: *mut vk::Semaphore) -> R {
    let mut init = 0;
    walk((*ci).p_next as *mut c_void, |t, p| {
        if t == vk::StructureType::SEMAPHORE_TYPE_CREATE_INFO {
            init = (*(p as *const vk::SemaphoreTypeCreateInfo<'_>)).initial_value;
        }
    });
    let h = obj::new_handle();
    objs().sem.insert(h, obj::Semaphore::new(init));
    *out = vk::Semaphore::from_raw(h);
    OK
}
unsafe extern "system" fn destroy_semaphore(_d: vk::Device, s: u64, _a: *const c_void) {
    // the worker may still write a pending signal into the word: it is leaked rather than freed
    if let Some(s) = objs().sem.remove(&s) {
        std::mem::forget(s);
    }
}
unsafe extern "system" fn semaphore_value(_d: vk::Device, s: vk::Semaphore, out: *mut u64) -> R {
    *out = objs().sem.get(&s.as_raw()).map_or(0, |s| s.word.get());
    OK
}
fn word_addr(h: u64) -> Option<&'static std::sync::atomic::AtomicU64> {
    // SAFETY: words are never freed (destroy_semaphore leaks them)
    objs().sem.get(&h).map(|s| unsafe { &*(s.word.addr() as *const std::sync::atomic::AtomicU64) })
}
fn queued_addr(h: u64) -> Option<&'static std::sync::atomic::AtomicU64> {
    // SAFETY: as `word_addr` (the queued word is leaked with its semaphore)
    objs().sem.get(&h).map(|s| unsafe { &*(s.queued.addr() as *const std::sync::atomic::AtomicU64) })
}

/// Waits until the worker has handed the submission that signals `value` on `sem` to the GPU, or has stopped: the
/// worker's CPU-side execute of a submission (command encoding, WebGPU pipelines created at their first draw) is what
/// a native driver does inside `vkQueueSubmit`, so a GPU-time budget starts after it. Returns the time waited.
pub fn wait_queued(sem: vk::Semaphore, value: u64) -> std::time::Duration {
    let t = std::time::Instant::now();
    if let Some(q) = queued_addr(sem.as_raw()) {
        ring::wait_until(u64::MAX, || q.load(Ordering::Acquire) >= value);
    }
    t.elapsed()
}

unsafe extern "system" fn wait_semaphores(_d: vk::Device, wi: *const vk::SemaphoreWaitInfo<'_>, timeout: u64) -> R {
    let wi = &*wi;
    let any = wi.flags.contains(vk::SemaphoreWaitFlags::ANY);
    let pairs: Vec<(&std::sync::atomic::AtomicU64, u64)> = sl(wi.p_semaphores, wi.semaphore_count)
        .iter()
        .zip(sl(wi.p_values, wi.semaphore_count))
        .filter_map(|(s, &v)| word_addr(s.as_raw()).map(|w| (w, v)))
        .collect();
    let ready = || {
        let mut it = pairs.iter().map(|(w, v)| w.load(Ordering::Acquire) >= *v);
        if any {
            it.any(|x| x)
        } else {
            it.all(|x| x)
        }
    };
    if ring::wait_until(timeout, ready) {
        OK
    } else {
        R::TIMEOUT
    }
}
unsafe extern "system" fn signal_semaphore(_d: vk::Device, si: *const vk::SemaphoreSignalInfo<'_>) -> R {
    if let Some(w) = word_addr((*si).semaphore.as_raw()) {
        if let Some(q) = queued_addr((*si).semaphore.as_raw()) {
            q.store((*si).value, Ordering::Release);
        }
        w.store((*si).value, Ordering::Release);
        ring::hdr(ring::H_GEN).fetch_add(1, Ordering::AcqRel);
        ring::wake(ring::hdr(ring::H_GEN));
    }
    OK
}
unsafe extern "system" fn create_fence(_d: vk::Device, ci: *const vk::FenceCreateInfo<'_>, _a: *const c_void, out: *mut vk::Fence) -> R {
    let on = (*ci).flags.contains(vk::FenceCreateFlags::SIGNALED) as u64;
    let h = obj::new_handle();
    objs().sem.insert(h, obj::Semaphore::new(on));
    *out = vk::Fence::from_raw(h);
    OK
}
unsafe extern "system" fn wait_fences(_d: vk::Device, n: u32, f: *const vk::Fence, all: vk::Bool32, timeout: u64) -> R {
    let words: Vec<&std::sync::atomic::AtomicU64> = sl(f, n).iter().filter_map(|f| word_addr(f.as_raw())).collect();
    let ready = || {
        let mut it = words.iter().map(|w| w.load(Ordering::Acquire) != 0);
        if all != 0 {
            it.all(|x| x)
        } else {
            it.any(|x| x)
        }
    };
    if ring::wait_until(timeout, ready) {
        OK
    } else {
        R::TIMEOUT
    }
}
unsafe extern "system" fn reset_fences(_d: vk::Device, n: u32, f: *const vk::Fence) -> R {
    for f in sl(f, n) {
        if let Some(w) = word_addr(f.as_raw()) {
            w.store(0, Ordering::Release);
        }
        if let Some(q) = queued_addr(f.as_raw()) {
            q.store(0, Ordering::Release);
        }
    }
    OK
}
unsafe extern "system" fn fence_status(_d: vk::Device, f: vk::Fence) -> R {
    match word_addr(f.as_raw()) {
        Some(w) if w.load(Ordering::Acquire) != 0 => OK,
        _ => R::NOT_READY,
    }
}

// ---------- queries ----------

unsafe extern "system" fn create_query_pool(_d: vk::Device, ci: *const vk::QueryPoolCreateInfo<'_>, _a: *const c_void, out: *mut vk::QueryPool) -> R {
    let h = obj::new_handle();
    objs().query.insert(h, obj::QueryPool { ty: (*ci).query_type, count: (*ci).query_count });
    *out = vk::QueryPool::from_raw(h);
    OK
}
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn query_results(
    _d: vk::Device,
    _q: vk::QueryPool,
    _first: u32,
    n: u32,
    size: usize,
    data: *mut c_void,
    stride: u64,
    flags: vk::QueryResultFlags,
) -> R {
    // the web device has no timestamps; occlusion queries are refused when recorded (`Cmd::Unsupported`)
    let _ = (n, stride, flags);
    std::ptr::write_bytes(data as *mut u8, 0, size);
    OK
}
unsafe extern "system" fn reset_query_pool_host(_d: vk::Device, _q: vk::QueryPool, _f: u32, _n: u32) {}

// ---------- entry points ----------

macro_rules! pf {
    ($f:expr) => {
        Some($f as *const ())
    };
}

fn lookup(name: &str) -> Option<*const ()> {
    let base = name.strip_suffix("EXT").or_else(|| name.strip_suffix("KHR")).unwrap_or(name);
    let f: Option<*const ()> = match name {
        "vkGetInstanceProcAddr" => pf!(gipa),
        "vkGetDeviceProcAddr" => pf!(gdpa),
        "vkCreateInstance" => pf!(create_instance),
        "vkDestroyInstance" => pf!(destroy_instance),
        "vkEnumerateInstanceExtensionProperties" => pf!(enum_instance_exts),
        "vkEnumerateInstanceLayerProperties" => pf!(enum_instance_layers),
        "vkEnumerateInstanceVersion" => pf!(enum_instance_version),
        "vkEnumeratePhysicalDevices" => pf!(enum_pdevs),
        "vkGetPhysicalDeviceProperties" => pf!(get_props),
        "vkGetPhysicalDeviceMemoryProperties" => pf!(get_mem_props),
        "vkGetPhysicalDeviceQueueFamilyProperties" => pf!(get_qfams),
        "vkGetPhysicalDeviceFeatures" => pf!(get_features),
        "vkEnumerateDeviceExtensionProperties" => pf!(enum_dev_exts),
        "vkGetPhysicalDeviceFormatProperties" => pf!(get_format_props),
        "vkGetPhysicalDeviceImageFormatProperties" => pf!(get_image_format_props),
        "vkGetPhysicalDeviceSparseImageFormatProperties" => pf!(get_sparse_props),
        "vkGetPhysicalDeviceSurfaceSupportKHR" => pf!(surface_support),
        "vkGetPhysicalDeviceSurfaceCapabilitiesKHR" => pf!(surface_caps),
        "vkGetPhysicalDeviceSurfaceFormatsKHR" => pf!(surface_formats),
        "vkGetPhysicalDeviceSurfacePresentModesKHR" => pf!(surface_modes),
        "vkDestroySurfaceKHR" => pf!(destroy_surface),
        "vkCreateSwapchainKHR" => pf!(create_swapchain),
        "vkDestroySwapchainKHR" => pf!(destroy_swapchain),
        "vkGetSwapchainImagesKHR" => pf!(swapchain_images),
        "vkAcquireNextImageKHR" => pf!(acquire_next_image),
        "vkQueuePresentKHR" => pf!(queue_present),
        "vkCreateDevice" => pf!(create_device),
        "vkDestroyDevice" => pf!(destroy_device),
        "vkGetDeviceQueue" => pf!(get_queue),
        "vkGetDeviceQueue2" => pf!(get_queue2),
        "vkDeviceWaitIdle" => pf!(device_wait_idle),
        "vkQueueWaitIdle" => pf!(queue_wait_idle),
        "vkQueueSubmit" => pf!(submit::queue_submit),
        "vkAllocateMemory" => pf!(allocate_memory),
        "vkFreeMemory" => pf!(free_memory),
        "vkMapMemory" => pf!(map_memory),
        "vkUnmapMemory" => pf!(unmap_memory),
        "vkFlushMappedMemoryRanges" | "vkInvalidateMappedMemoryRanges" => pf!(flush_ranges),
        "vkCreateBuffer" => pf!(create_buffer),
        "vkDestroyBuffer" => pf!(destroy_buffer),
        "vkGetBufferMemoryRequirements" => pf!(buffer_mem_reqs),
        "vkBindBufferMemory" => pf!(bind_buffer_memory),
        "vkCreateBufferView" => pf!(create_buffer_view),
        "vkDestroyBufferView" | "vkDestroyCommandPool" | "vkDestroyPipelineLayout" | "vkDestroyDescriptorSetLayout"
        | "vkDestroyPipelineCache" | "vkDestroyQueryPool" => pf!(destroy_nothing),
        "vkCreateImage" => pf!(create_image),
        "vkDestroyImage" => pf!(destroy_image),
        "vkGetImageMemoryRequirements" => pf!(image_mem_reqs),
        "vkBindImageMemory" => pf!(bind_image_memory),
        "vkGetImageSubresourceLayout" => pf!(subresource_layout),
        "vkCreateImageView" => pf!(create_image_view),
        "vkDestroyImageView" => pf!(destroy_image_view),
        "vkCreateSampler" => pf!(create_sampler),
        "vkDestroySampler" => pf!(destroy_sampler),
        "vkCreateShaderModule" => pf!(create_shader_module),
        "vkDestroyShaderModule" => pf!(destroy_shader_module),
        "vkCreateDescriptorSetLayout" => pf!(create_dsl),
        "vkCreatePipelineLayout" => pf!(create_playout),
        "vkCreateDescriptorPool" => pf!(create_dpool),
        "vkDestroyDescriptorPool" => pf!(destroy_dpool),
        "vkResetDescriptorPool" => pf!(reset_dpool),
        "vkAllocateDescriptorSets" => pf!(alloc_dsets),
        "vkFreeDescriptorSets" => pf!(free_dsets),
        "vkUpdateDescriptorSets" => pf!(update_dsets),
        "vkCreatePipelineCache" => pf!(create_pcache),
        "vkGetPipelineCacheData" => pf!(pcache_data),
        "vkMergePipelineCaches" => pf!(merge_pcaches),
        "vkCreateGraphicsPipelines" => pf!(create_gfx_pipelines),
        "vkCreateComputePipelines" => pf!(create_compute_pipelines),
        "vkDestroyPipeline" => pf!(destroy_pipeline),
        "vkCreateCommandPool" => pf!(create_cpool),
        "vkResetCommandPool" => pf!(reset_cpool),
        "vkAllocateCommandBuffers" => pf!(alloc_cbs),
        "vkFreeCommandBuffers" => pf!(free_cbs),
        "vkCreateSemaphore" => pf!(create_semaphore),
        "vkDestroySemaphore" | "vkDestroyFence" => pf!(destroy_semaphore),
        "vkCreateFence" => pf!(create_fence),
        "vkWaitForFences" => pf!(wait_fences),
        "vkResetFences" => pf!(reset_fences),
        "vkGetFenceStatus" => pf!(fence_status),
        "vkCreateQueryPool" => pf!(create_query_pool),
        "vkGetQueryPoolResults" => pf!(query_results),
        "vkBeginCommandBuffer" => pf!(cmd::begin),
        "vkEndCommandBuffer" => pf!(cmd::end),
        "vkResetCommandBuffer" => pf!(cmd::reset),
        "vkCmdBindPipeline" => pf!(cmd::bind_pipeline),
        "vkCmdBindDescriptorSets" => pf!(cmd::bind_descriptor_sets),
        "vkCmdPushConstants" => pf!(cmd::push_constants),
        "vkCmdBindVertexBuffers" => pf!(cmd::bind_vertex_buffers),
        "vkCmdBindIndexBuffer" => pf!(cmd::bind_index_buffer),
        "vkCmdDraw" => pf!(cmd::draw),
        "vkCmdDrawIndexed" => pf!(cmd::draw_indexed),
        "vkCmdDrawIndirect" => pf!(cmd::draw_indirect),
        "vkCmdDrawIndexedIndirect" => pf!(cmd::draw_indexed_indirect),
        "vkCmdDispatch" => pf!(cmd::dispatch),
        "vkCmdDispatchIndirect" => pf!(cmd::dispatch_indirect),
        "vkCmdCopyBuffer" => pf!(cmd::copy_buffer),
        "vkCmdCopyBufferToImage" => pf!(cmd::copy_buffer_to_image),
        "vkCmdCopyImageToBuffer" => pf!(cmd::copy_image_to_buffer),
        "vkCmdCopyImage" => pf!(cmd::copy_image),
        "vkCmdBlitImage" => pf!(cmd::blit_image),
        "vkCmdClearColorImage" => pf!(cmd::clear_color_image),
        "vkCmdClearDepthStencilImage" => pf!(cmd::clear_depth_stencil_image),
        "vkCmdClearAttachments" => pf!(cmd::clear_attachments),
        "vkCmdFillBuffer" => pf!(cmd::fill_buffer),
        "vkCmdUpdateBuffer" => pf!(cmd::update_buffer),
        "vkCmdPipelineBarrier" => pf!(cmd::nop_barrier),
        "vkCmdBeginQuery" => pf!(cmd::begin_query),
        "vkCmdEndQuery" => pf!(cmd::end_query),
        "vkCmdResetQueryPool" => pf!(cmd::reset_query_pool),
        "vkCmdSetViewport" => pf!(cmd::set_viewport),
        "vkCmdSetScissor" => pf!(cmd::set_scissor),
        "vkCmdSetDepthBias" => pf!(cmd::set_depth_bias),
        "vkCmdSetBlendConstants" => pf!(cmd::set_blend_constants),
        "vkCmdSetLineWidth" => pf!(cmd::set_line_width),
        "vkCmdSetDepthBounds" => pf!(cmd::set_depth_bounds),
        "vkCmdSetStencilCompareMask" => pf!(cmd::set_stencil_compare_mask),
        "vkCmdSetStencilWriteMask" => pf!(cmd::set_stencil_write_mask),
        "vkCmdSetStencilReference" => pf!(cmd::set_stencil_reference),
        "vkResetQueryPool" => pf!(reset_query_pool_host),
        _ => None,
    };
    if f.is_some() {
        return f;
    }
    // core 1.1-1.3 names and their extension aliases
    match base {
        "vkGetPhysicalDeviceProperties2" => pf!(get_props2),
        "vkGetPhysicalDeviceMemoryProperties2" => pf!(get_mem_props2),
        "vkGetPhysicalDeviceQueueFamilyProperties2" => pf!(get_qfams2),
        "vkGetPhysicalDeviceFeatures2" => pf!(get_features2),
        "vkGetPhysicalDeviceFormatProperties2" => pf!(get_format_props2),
        "vkGetBufferMemoryRequirements2" => pf!(buffer_mem_reqs2),
        "vkGetImageMemoryRequirements2" => pf!(image_mem_reqs2),
        "vkGetDeviceBufferMemoryRequirements" => pf!(device_buffer_mem_reqs),
        "vkBindBufferMemory2" => pf!(bind_buffer_memory2),
        "vkBindImageMemory2" => pf!(bind_image_memory2),
        "vkGetSemaphoreCounterValue" => pf!(semaphore_value),
        "vkWaitSemaphores" => pf!(wait_semaphores),
        "vkSignalSemaphore" => pf!(signal_semaphore),
        "vkQueueSubmit2" => pf!(submit::queue_submit2),
        "vkCmdPipelineBarrier2" => pf!(cmd::nop_barrier2),
        "vkCmdWriteTimestamp2" => pf!(cmd::write_timestamp2),
        "vkCmdBeginRendering" => pf!(cmd::begin_rendering),
        "vkCmdEndRendering" => pf!(cmd::end_rendering),
        "vkCmdBindVertexBuffers2" => pf!(cmd::bind_vertex_buffers2),
        "vkCmdCopyBuffer2" => pf!(cmd::copy_buffer2),
        "vkCmdCopyBufferToImage2" => pf!(cmd::copy_buffer_to_image2),
        "vkCmdCopyImageToBuffer2" => pf!(cmd::copy_image_to_buffer2),
        "vkCmdCopyImage2" => pf!(cmd::copy_image2),
        "vkCmdBlitImage2" => pf!(cmd::blit_image2),
        "vkCmdSetViewportWithCount" => pf!(cmd::set_viewport_with_count),
        "vkCmdSetScissorWithCount" => pf!(cmd::set_scissor_with_count),
        "vkCmdSetPrimitiveTopology" => pf!(cmd::set_topology),
        "vkCmdSetPrimitiveRestartEnable" => pf!(cmd::set_restart),
        "vkCmdSetCullMode" => pf!(cmd::set_cull),
        "vkCmdSetFrontFace" => pf!(cmd::set_front),
        "vkCmdSetDepthTestEnable" => pf!(cmd::set_depth_test),
        "vkCmdSetDepthWriteEnable" => pf!(cmd::set_depth_write),
        "vkCmdSetDepthCompareOp" => pf!(cmd::set_depth_compare),
        "vkCmdSetDepthBoundsTestEnable" => pf!(cmd::set_depth_bounds_test),
        "vkCmdSetStencilTestEnable" => pf!(cmd::set_stencil_test),
        "vkCmdSetStencilOp" => pf!(cmd::set_stencil_op),
        "vkCmdSetDepthBiasEnable" => pf!(cmd::set_depth_bias_enable),
        "vkCmdSetRasterizerDiscardEnable" => pf!(cmd::set_rasterizer_discard),
        "vkCmdSetDepthClampEnable" => pf!(cmd::set_depth_clamp),
        "vkCmdSetPolygonMode" => pf!(cmd::set_polygon_mode),
        "vkCmdSetRasterizationSamples" => pf!(cmd::set_samples),
        "vkCmdSetSampleMask" => pf!(cmd::set_sample_mask),
        "vkCmdSetAlphaToCoverageEnable" => pf!(cmd::set_a2c),
        "vkCmdSetLogicOpEnable" => pf!(cmd::set_logic_op_enable),
        "vkCmdSetLogicOp" => pf!(cmd::set_logic_op),
        "vkCmdSetColorBlendEnable" => pf!(cmd::set_blend_enable),
        "vkCmdSetColorBlendEquation" => pf!(cmd::set_blend_equation),
        "vkCmdSetColorWriteMask" => pf!(cmd::set_write_mask),
        "vkCmdSetDepthClipNegativeOneToOne" => pf!(cmd::set_clip_neg),
        "vkCmdSetPatchControlPoints" => pf!(cmd::set_patch_points),
        "vkCmdSetConservativeRasterizationMode" => pf!(cmd::set_conservative),
        "vkCmdSetVertexInput" => pf!(cmd::set_vertex_input),
        "vkCmdDrawIndirectCount" | "vkCmdDrawIndexedIndirectCount" => pf!(cmd::draw_indirect_count),
        "vkCmdBeginDebugUtilsLabel" | "vkCmdInsertDebugUtilsLabel" => pf!(cmd::nop_label),
        "vkCmdEndDebugUtilsLabel" => pf!(cmd::nop_end_label),
        _ => None,
    }
}

pub unsafe extern "system" fn gipa(_i: vk::Instance, name: *const c_char) -> vk::PFN_vkVoidFunction {
    let n = CStr::from_ptr(name).to_str().unwrap_or("");
    // SAFETY: each entry is an `unsafe extern "system" fn` of the signature Vulkan gives that name
    lookup(n).map(|p| std::mem::transmute::<*const (), unsafe extern "system" fn()>(p))
}
unsafe extern "system" fn gdpa(_d: vk::Device, name: *const c_char) -> vk::PFN_vkVoidFunction {
    gipa(vk::Instance::null(), name)
}

/// The Vulkan entry the replay loads its function tables from.
pub fn entry() -> ash::Entry {
    // SAFETY: gipa answers every name with a function of that name's signature, or none
    unsafe { ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: gipa }) }
}

pub fn surface() -> vk::SurfaceKHR {
    vk::SurfaceKHR::from_raw(SURFACE)
}

/// The image behind a handle (for `submit`).
pub fn image(h: u64) -> Option<Image> {
    objs().img.get(&h).cloned()
}
pub fn view(h: u64) -> Option<View> {
    objs().view.get(&h).cloned()
}

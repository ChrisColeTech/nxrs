//! Instance, physical device choice, logical device with the features the replay relies on, and the validation
//! message counters the tests read.

use ash::ext::debug_utils;
use ash::vk;
use std::ffi::{c_void, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};

pub static VALIDATION_ERRORS: AtomicU64 = AtomicU64::new(0);
pub static VALIDATION_WARNINGS: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn debug_cb(
    sev: vk::DebugUtilsMessageSeverityFlagsEXT,
    ty: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    let msg = if data.is_null() || (*data).p_message.is_null() {
        String::from("?")
    } else {
        CStr::from_ptr((*data).p_message).to_string_lossy().into_owned()
    };
    if sev.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        VALIDATION_ERRORS.fetch_add(1, Ordering::Relaxed);
        crate::log::red(&format!("[vk] validation error: {msg}"));
    } else if sev.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) && !ty.contains(vk::DebugUtilsMessageTypeFlagsEXT::GENERAL) {
        VALIDATION_WARNINGS.fetch_add(1, Ordering::Relaxed);
        // performance notes (GENERAL layouts are the design here) are counted, not logged
        if !ty.contains(vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE) {
            crate::vklog!("validation warning: {msg}");
        }
    }
    vk::FALSE
}

/// What the device offers that the replay picks paths by.
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    pub host_import: bool,
    pub import_align: u64,
    pub divisor: bool,
    pub index_u8: bool,
    pub custom_border: bool,
    pub eds3_samples: bool,
    pub eds3_clip_neg: bool,
    pub eds3_clamp: bool,
    pub eds3_poly: bool,
    pub eds3_a2c: bool,
    pub eds3_logic_en: bool,
    pub eds3_sample_mask: bool,
    pub logic_op_dyn: bool,
    pub patch_dyn: bool,
    pub timestamp_period: f32,
    pub uniform_align: u64,
    pub storage_align: u64,
}

pub struct Device {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub debug: Option<(debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub pdev: vk::PhysicalDevice,
    pub dev: ash::Device,
    pub qfam: u32,
    pub queue: vk::Queue,
    pub props: vk::PhysicalDeviceProperties,
    pub mem_props: vk::PhysicalDeviceMemoryProperties,
    pub caps: Caps,
    pub name: String,
    pub surface_i: ash::khr::surface::Instance,
    pub win32_i: ash::khr::win32_surface::Instance,
    pub swapchain: ash::khr::swapchain::Device,
    pub eds3: ash::ext::extended_dynamic_state3::Device,
    pub eds2: ash::ext::extended_dynamic_state2::Device,
    pub vin: ash::ext::vertex_input_dynamic_state::Device,
    pub emh: ash::ext::external_memory_host::Device,
}

fn env_on(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|v| v != "0" && !v.is_empty())
}

impl Device {
    pub fn new() -> Result<Device, String> {
        let entry = unsafe { ash::Entry::load() }.map_err(|e| format!("Vulkan loader: {e}"))?;
        let app = CString::new("RecompLauncherEngine").unwrap();
        let app_info = vk::ApplicationInfo::default().application_name(&app).engine_name(&app).api_version(vk::API_VERSION_1_3);
        let layers_avail: Vec<String> = unsafe { entry.enumerate_instance_layer_properties() }
            .unwrap_or_default()
            .iter()
            .map(|l| unsafe { CStr::from_ptr(l.layer_name.as_ptr()) }.to_string_lossy().into_owned())
            .collect();
        let want_validation = env_on("NXGFX_VALIDATION").unwrap_or(false);
        let val_name = CString::new("VK_LAYER_KHRONOS_validation").unwrap();
        let mut layers = vec![];
        if want_validation {
            if layers_avail.iter().any(|l| l == "VK_LAYER_KHRONOS_validation") {
                layers.push(val_name.as_ptr());
            } else {
                crate::log::red("NXGFX_VALIDATION is set but VK_LAYER_KHRONOS_validation is not installed");
            }
        }
        let mut exts = vec![
            ash::khr::surface::NAME.as_ptr(),
            ash::khr::win32_surface::NAME.as_ptr(),
            ash::khr::get_surface_capabilities2::NAME.as_ptr(),
        ];
        let inst_exts: Vec<String> = unsafe { entry.enumerate_instance_extension_properties(None) }
            .unwrap_or_default()
            .iter()
            .map(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) }.to_string_lossy().into_owned())
            .collect();
        let has_debug = inst_exts.iter().any(|e| e == "VK_EXT_debug_utils");
        if has_debug {
            exts.push(debug_utils::NAME.as_ptr());
        }
        exts.retain(|e| {
            let s = unsafe { CStr::from_ptr(*e) }.to_string_lossy().into_owned();
            inst_exts.contains(&s)
        });
        let mut dbg_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
            .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING)
            .message_type(
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
            )
            .pfn_user_callback(Some(debug_cb));
        let mut ci = vk::InstanceCreateInfo::default().application_info(&app_info).enabled_layer_names(&layers).enabled_extension_names(&exts);
        if has_debug && !layers.is_empty() {
            ci = ci.push_next(&mut dbg_info);
        }
        let instance = unsafe { entry.create_instance(&ci, None) }.map_err(|e| format!("vkCreateInstance: {e}"))?;
        let debug = if has_debug && !layers.is_empty() {
            let du = debug_utils::Instance::new(&entry, &instance);
            let m = unsafe { du.create_debug_utils_messenger(&dbg_info, None) }.map_err(|e| format!("debug messenger: {e}"))?;
            Some((du, m))
        } else {
            None
        };

        // The device: NXGFX_GPU names one (substring of its name), else the discrete GPU with the most local memory.
        let pdevs = unsafe { instance.enumerate_physical_devices() }.map_err(|e| format!("{e}"))?;
        let want = std::env::var("NXGFX_GPU").ok();
        let mut best: Option<(vk::PhysicalDevice, u64)> = None;
        for &p in &pdevs {
            let pr = unsafe { instance.get_physical_device_properties(p) };
            if pr.api_version < vk::API_VERSION_1_3 {
                continue;
            }
            let name = unsafe { CStr::from_ptr(pr.device_name.as_ptr()) }.to_string_lossy().into_owned();
            let mp = unsafe { instance.get_physical_device_memory_properties(p) };
            let local: u64 = mp.memory_heaps[..mp.memory_heap_count as usize]
                .iter()
                .filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
                .map(|h| h.size)
                .sum();
            let mut score = local;
            if pr.device_type == vk::PhysicalDeviceType::DISCRETE_GPU {
                score += 1 << 50;
            }
            if let Some(w) = &want {
                if name.to_lowercase().contains(&w.to_lowercase()) {
                    score += 1 << 60;
                }
            }
            if best.is_none_or(|b| score > b.1) {
                best = Some((p, score));
            }
        }
        let pdev = best.ok_or("no Vulkan 1.3 device")?.0;
        let props = unsafe { instance.get_physical_device_properties(pdev) };
        let mem_props = unsafe { instance.get_physical_device_memory_properties(pdev) };
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy().into_owned();
        let qfams = unsafe { instance.get_physical_device_queue_family_properties(pdev) };
        let qfam = qfams
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE))
            .ok_or("no graphics queue")? as u32;

        let dev_exts: Vec<String> = unsafe { instance.enumerate_device_extension_properties(pdev) }
            .unwrap_or_default()
            .iter()
            .map(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) }.to_string_lossy().into_owned())
            .collect();
        let has = |n: &CStr| dev_exts.iter().any(|e| e.as_str() == n.to_str().unwrap());
        let required: [&CStr; 5] = [
            ash::khr::swapchain::NAME,
            ash::ext::extended_dynamic_state2::NAME,
            ash::ext::extended_dynamic_state3::NAME,
            ash::ext::vertex_input_dynamic_state::NAME,
            ash::ext::depth_clip_control::NAME,
        ];
        for r in required {
            if !has(r) {
                return Err(format!("device {name} lacks {}", r.to_string_lossy()));
            }
        }
        let mut dexts: Vec<*const i8> = required.iter().map(|n| n.as_ptr()).collect();
        let optional: [&CStr; 5] = [
            ash::ext::external_memory_host::NAME,
            ash::ext::vertex_attribute_divisor::NAME,
            ash::ext::index_type_uint8::NAME,
            ash::ext::custom_border_color::NAME,
            ash::ext::fragment_shader_interlock::NAME,
        ];
        for o in optional {
            if has(o) {
                dexts.push(o.as_ptr());
            }
        }

        // Features: query what the extensions offer, enable what the replay uses.
        let mut f_eds2 = vk::PhysicalDeviceExtendedDynamicState2FeaturesEXT::default();
        let mut f_eds3 = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default();
        let mut f_vin = vk::PhysicalDeviceVertexInputDynamicStateFeaturesEXT::default();
        let mut f_dcc = vk::PhysicalDeviceDepthClipControlFeaturesEXT::default();
        let mut f_div = vk::PhysicalDeviceVertexAttributeDivisorFeaturesEXT::default();
        let mut f_u8 = vk::PhysicalDeviceIndexTypeUint8FeaturesEXT::default();
        let mut f_cb = vk::PhysicalDeviceCustomBorderColorFeaturesEXT::default();
        let mut f_il = vk::PhysicalDeviceFragmentShaderInterlockFeaturesEXT::default();
        let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut f11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut f2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut f11)
            .push_next(&mut f12)
            .push_next(&mut f13)
            .push_next(&mut f_eds2)
            .push_next(&mut f_eds3)
            .push_next(&mut f_vin)
            .push_next(&mut f_dcc)
            .push_next(&mut f_div)
            .push_next(&mut f_u8)
            .push_next(&mut f_cb)
            .push_next(&mut f_il);
        unsafe { instance.get_physical_device_features2(pdev, &mut f2) };
        let base = f2.features;
        let need = |ok: u32, what: &str| -> Result<(), String> { if ok != 0 { Ok(()) } else { Err(format!("device {name} lacks {what}")) } };
        need(f13.dynamic_rendering, "dynamicRendering")?;
        need(f13.synchronization2, "synchronization2")?;
        need(f12.timeline_semaphore, "timelineSemaphore")?;
        need(f12.descriptor_binding_partially_bound, "descriptorBindingPartiallyBound")?;
        need(f12.descriptor_binding_sampled_image_update_after_bind, "descriptorBindingSampledImageUpdateAfterBind")?;
        need(f12.descriptor_binding_storage_image_update_after_bind, "descriptorBindingStorageImageUpdateAfterBind")?;
        need(f12.runtime_descriptor_array, "runtimeDescriptorArray")?;
        need(f_vin.vertex_input_dynamic_state, "vertexInputDynamicState")?;
        need(f_dcc.depth_clip_control, "depthClipControl")?;
        need(f_eds3.extended_dynamic_state3_color_blend_enable, "extendedDynamicState3ColorBlendEnable")?;
        need(f_eds3.extended_dynamic_state3_color_blend_equation, "extendedDynamicState3ColorBlendEquation")?;
        need(f_eds3.extended_dynamic_state3_color_write_mask, "extendedDynamicState3ColorWriteMask")?;

        let caps_eds3 = f_eds3;
        let mut caps = Caps {
            divisor: has(ash::ext::vertex_attribute_divisor::NAME) && f_div.vertex_attribute_instance_rate_divisor != 0,
            index_u8: has(ash::ext::index_type_uint8::NAME) && f_u8.index_type_uint8 != 0,
            custom_border: has(ash::ext::custom_border_color::NAME) && f_cb.custom_border_colors != 0,
            eds3_samples: caps_eds3.extended_dynamic_state3_rasterization_samples != 0,
            eds3_clip_neg: caps_eds3.extended_dynamic_state3_depth_clip_negative_one_to_one != 0,
            eds3_clamp: caps_eds3.extended_dynamic_state3_depth_clamp_enable != 0 && base.depth_clamp != 0,
            eds3_poly: caps_eds3.extended_dynamic_state3_polygon_mode != 0 && base.fill_mode_non_solid != 0,
            eds3_a2c: caps_eds3.extended_dynamic_state3_alpha_to_coverage_enable != 0,
            eds3_logic_en: caps_eds3.extended_dynamic_state3_logic_op_enable != 0 && base.logic_op != 0,
            eds3_sample_mask: caps_eds3.extended_dynamic_state3_sample_mask != 0,
            logic_op_dyn: f_eds2.extended_dynamic_state2_logic_op != 0 && base.logic_op != 0,
            patch_dyn: f_eds2.extended_dynamic_state2_patch_control_points != 0 && base.tessellation_shader != 0,
            timestamp_period: props.limits.timestamp_period,
            uniform_align: props.limits.min_uniform_buffer_offset_alignment,
            storage_align: props.limits.min_storage_buffer_offset_alignment,
            ..Default::default()
        };
        if has(ash::ext::external_memory_host::NAME) {
            let mut emp = vk::PhysicalDeviceExternalMemoryHostPropertiesEXT::default();
            let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut emp);
            unsafe { instance.get_physical_device_properties2(pdev, &mut p2) };
            caps.host_import = true;
            caps.import_align = emp.min_imported_host_pointer_alignment;
        }

        // Enable: exactly the queried support for the parts we use.
        let en_base = vk::PhysicalDeviceFeatures {
            independent_blend: base.independent_blend,
            geometry_shader: base.geometry_shader,
            tessellation_shader: base.tessellation_shader,
            dual_src_blend: base.dual_src_blend,
            logic_op: base.logic_op,
            multi_draw_indirect: base.multi_draw_indirect,
            depth_clamp: base.depth_clamp,
            depth_bias_clamp: base.depth_bias_clamp,
            fill_mode_non_solid: base.fill_mode_non_solid,
            wide_lines: base.wide_lines,
            large_points: base.large_points,
            sampler_anisotropy: base.sampler_anisotropy,
            texture_compression_bc: base.texture_compression_bc,
            occlusion_query_precise: base.occlusion_query_precise,
            vertex_pipeline_stores_and_atomics: base.vertex_pipeline_stores_and_atomics,
            fragment_stores_and_atomics: base.fragment_stores_and_atomics,
            shader_image_gather_extended: base.shader_image_gather_extended,
            shader_storage_image_extended_formats: base.shader_storage_image_extended_formats,
            shader_storage_image_read_without_format: base.shader_storage_image_read_without_format,
            shader_storage_image_write_without_format: base.shader_storage_image_write_without_format,
            shader_clip_distance: base.shader_clip_distance,
            shader_cull_distance: base.shader_cull_distance,
            shader_float64: base.shader_float64,
            shader_int64: base.shader_int64,
            shader_int16: base.shader_int16,
            image_cube_array: base.image_cube_array,
            sample_rate_shading: base.sample_rate_shading,
            ..Default::default()
        };
        let mut e11 = vk::PhysicalDeviceVulkan11Features::default().shader_draw_parameters(f11.shader_draw_parameters != 0);
        let mut e12 = vk::PhysicalDeviceVulkan12Features::default()
            .timeline_semaphore(true)
            .descriptor_indexing(true)
            .descriptor_binding_partially_bound(true)
            .descriptor_binding_sampled_image_update_after_bind(true)
            .descriptor_binding_storage_image_update_after_bind(true)
            .descriptor_binding_uniform_texel_buffer_update_after_bind(f12.descriptor_binding_uniform_texel_buffer_update_after_bind != 0)
            .descriptor_binding_storage_texel_buffer_update_after_bind(f12.descriptor_binding_storage_texel_buffer_update_after_bind != 0)
            .descriptor_binding_update_unused_while_pending(f12.descriptor_binding_update_unused_while_pending != 0)
            .runtime_descriptor_array(true)
            .shader_sampled_image_array_non_uniform_indexing(f12.shader_sampled_image_array_non_uniform_indexing != 0)
            .shader_storage_image_array_non_uniform_indexing(f12.shader_storage_image_array_non_uniform_indexing != 0)
            .scalar_block_layout(f12.scalar_block_layout != 0)
            .shader_float16(f12.shader_float16 != 0)
            .shader_int8(f12.shader_int8 != 0)
            .storage_buffer8_bit_access(f12.storage_buffer8_bit_access != 0)
            .uniform_and_storage_buffer8_bit_access(f12.uniform_and_storage_buffer8_bit_access != 0)
            .host_query_reset(f12.host_query_reset != 0)
            .sampler_filter_minmax(f12.sampler_filter_minmax != 0)
            .sampler_mirror_clamp_to_edge(f12.sampler_mirror_clamp_to_edge != 0);
        let mut e13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true)
            .maintenance4(f13.maintenance4 != 0)
            .shader_demote_to_helper_invocation(f13.shader_demote_to_helper_invocation != 0);
        let mut e_eds2 = vk::PhysicalDeviceExtendedDynamicState2FeaturesEXT::default()
            .extended_dynamic_state2(true)
            .extended_dynamic_state2_logic_op(caps.logic_op_dyn)
            .extended_dynamic_state2_patch_control_points(caps.patch_dyn);
        let mut e_eds3 = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default()
            .extended_dynamic_state3_color_blend_enable(true)
            .extended_dynamic_state3_color_blend_equation(true)
            .extended_dynamic_state3_color_write_mask(true)
            .extended_dynamic_state3_rasterization_samples(caps.eds3_samples)
            .extended_dynamic_state3_sample_mask(caps.eds3_sample_mask)
            .extended_dynamic_state3_depth_clip_negative_one_to_one(caps.eds3_clip_neg)
            .extended_dynamic_state3_depth_clamp_enable(caps.eds3_clamp)
            .extended_dynamic_state3_polygon_mode(caps.eds3_poly)
            .extended_dynamic_state3_alpha_to_coverage_enable(caps.eds3_a2c)
            .extended_dynamic_state3_logic_op_enable(caps.eds3_logic_en);
        let mut e_vin = vk::PhysicalDeviceVertexInputDynamicStateFeaturesEXT::default().vertex_input_dynamic_state(true);
        let mut e_dcc = vk::PhysicalDeviceDepthClipControlFeaturesEXT::default().depth_clip_control(true);
        let mut e_div = vk::PhysicalDeviceVertexAttributeDivisorFeaturesEXT::default()
            .vertex_attribute_instance_rate_divisor(caps.divisor)
            .vertex_attribute_instance_rate_zero_divisor(caps.divisor && f_div.vertex_attribute_instance_rate_zero_divisor != 0);
        let mut e_u8 = vk::PhysicalDeviceIndexTypeUint8FeaturesEXT::default().index_type_uint8(caps.index_u8);
        let mut e_cb = vk::PhysicalDeviceCustomBorderColorFeaturesEXT::default()
            .custom_border_colors(caps.custom_border)
            .custom_border_color_without_format(caps.custom_border && f_cb.custom_border_color_without_format != 0);
        // the translator's fragment shaders order their framebuffer reads with pixel interlock
        let mut e_il = vk::PhysicalDeviceFragmentShaderInterlockFeaturesEXT::default()
            .fragment_shader_pixel_interlock(f_il.fragment_shader_pixel_interlock != 0)
            .fragment_shader_sample_interlock(f_il.fragment_shader_sample_interlock != 0);
        let prio = [1.0f32];
        let qci = [vk::DeviceQueueCreateInfo::default().queue_family_index(qfam).queue_priorities(&prio)];
        let mut dci = vk::DeviceCreateInfo::default()
            .queue_create_infos(&qci)
            .enabled_extension_names(&dexts)
            .enabled_features(&en_base)
            .push_next(&mut e11)
            .push_next(&mut e12)
            .push_next(&mut e13)
            .push_next(&mut e_eds2)
            .push_next(&mut e_eds3)
            .push_next(&mut e_vin)
            .push_next(&mut e_dcc);
        if has(ash::ext::vertex_attribute_divisor::NAME) {
            dci = dci.push_next(&mut e_div);
        }
        if has(ash::ext::index_type_uint8::NAME) {
            dci = dci.push_next(&mut e_u8);
        }
        if has(ash::ext::custom_border_color::NAME) {
            dci = dci.push_next(&mut e_cb);
        }
        if has(ash::ext::fragment_shader_interlock::NAME) {
            dci = dci.push_next(&mut e_il);
        }
        let dev = unsafe { instance.create_device(pdev, &dci, None) }.map_err(|e| format!("vkCreateDevice: {e}"))?;
        let queue = unsafe { dev.get_device_queue(qfam, 0) };
        crate::vklog!(
            "device {name} (Vulkan {}.{}.{}), validation {}, host import {} (align {:#x}), divisor {}, index u8 {}",
            vk::api_version_major(props.api_version),
            vk::api_version_minor(props.api_version),
            vk::api_version_patch(props.api_version),
            if debug.is_some() { "on" } else { "off" },
            caps.host_import,
            caps.import_align,
            caps.divisor,
            caps.index_u8
        );
        Ok(Device {
            surface_i: ash::khr::surface::Instance::new(&entry, &instance),
            win32_i: ash::khr::win32_surface::Instance::new(&entry, &instance),
            swapchain: ash::khr::swapchain::Device::new(&instance, &dev),
            eds3: ash::ext::extended_dynamic_state3::Device::new(&instance, &dev),
            eds2: ash::ext::extended_dynamic_state2::Device::new(&instance, &dev),
            vin: ash::ext::vertex_input_dynamic_state::Device::new(&instance, &dev),
            emh: ash::ext::external_memory_host::Device::new(&instance, &dev),
            entry,
            instance,
            debug,
            pdev,
            dev,
            qfam,
            queue,
            props,
            mem_props,
            caps,
            name,
        })
    }

    pub fn format_features(&self, f: vk::Format) -> vk::FormatFeatureFlags {
        unsafe { self.instance.get_physical_device_format_properties(self.pdev, f) }.optimal_tiling_features
    }

    pub fn mem_type(&self, bits: u32, want: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.mem_props.memory_type_count).find(|&i| bits & (1 << i) != 0 && self.mem_props.memory_types[i as usize].property_flags.contains(want))
    }
}

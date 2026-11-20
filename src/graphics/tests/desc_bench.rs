//! Measurements on this machine's GPU (ignored tests): classic descriptor sets (allocate + write) and descriptor-buffer
//! writes for a Pokemon Legends Z-A-sized frame, and NVN2's device integers read from a game's sdk.
//! `cargo test --release -p graphics --test desc_bench -- --ignored --nocapture --test-threads=1`.
use ash::vk;
use gpu_allocator::MemoryLocation;

fn gpu() -> &'static nxgfx::vk::Gpu {
    let mut dev = nxgfx::nvn::types::NVNdevice::boxed();
    let mut db = nxgfx::nvn::types::NVNdeviceBuilder::boxed();
    unsafe {
        nxgfx::nvn::device::nvnDeviceBuilderSetDefaults(&mut *db);
        assert_eq!(nxgfx::nvn::device::nvnDeviceInitialize(&mut *dev, &*db), 1);
    }
    std::mem::forget(dev);
    nxgfx::vk::Gpu::get().unwrap()
}

#[test]
#[ignore]
fn classic_sets_per_frame() {
    // classic sets need layouts without the descriptor-buffer flag
    std::env::set_var("NXGFX_DESCBUF", "0");
    let g = gpu();
    let buf = g.buffer(64 << 20, vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER, MemoryLocation::CpuToGpu, "bench").unwrap();
    let n_sets = 2170;
    for round in 0..4 {
        nxgfx::vk::queue::oneoff(g, |g, sub, _| {
            let t0 = std::time::Instant::now();
            let mut alloc_ns = 0u128;
            for i in 0..n_sets {
                let ta = std::time::Instant::now();
                let set = sub.alloc_set(g, g.layout.stage_sets[(i & 1) as usize]);
                alloc_ns += ta.elapsed().as_nanos();
                // 12 uniform banks, the support buffer, 2 storage buffers (a typical stage)
                let infos: Vec<vk::DescriptorBufferInfo> = (0..15).map(|k| vk::DescriptorBufferInfo { buffer: buf.buf, offset: ((i * 16 + k) as u64 * 256) % (60 << 20), range: 256 }).collect();
                let writes: Vec<vk::WriteDescriptorSet> = infos
                    .iter()
                    .enumerate()
                    .map(|(k, inf)| {
                        let (b, ty) = if k < 13 { (k as u32, vk::DescriptorType::UNIFORM_BUFFER) } else { (32 + k as u32 - 13, vk::DescriptorType::STORAGE_BUFFER) };
                        vk::WriteDescriptorSet::default().dst_set(set).dst_binding(if k == 12 { 31 } else { b }).descriptor_type(ty).buffer_info(std::slice::from_ref(inf))
                    })
                    .collect();
                unsafe { g.dev().update_descriptor_sets(&writes, &[]) };
                unsafe { g.dev().cmd_bind_descriptor_sets(sub.main, vk::PipelineBindPoint::GRAPHICS, g.layout.pipeline, (i & 1) as u32, &[set], &[]) };
            }
            let t = t0.elapsed();
            println!("round {round}: {n_sets} sets in {:.2} ms (allocation {:.2} ms)", t.as_secs_f64() * 1e3, alloc_ns as f64 / 1e6);
        });
        nxgfx::vk::queue::idle();
    }
}

#[test]
#[ignore]
fn nvn2_device_items_from_sdk() {
    // NXGFX_TEST_SDK=<a Switch 2 game's exefs/sdk>
    let Ok(p) = std::env::var("NXGFX_TEST_SDK") else { return };
    let raw = std::fs::read(p).unwrap();
    let img = nxrt::loader::parse(&raw).unwrap();
    nxgfx::nvn::device::SWITCH2.store(true, std::sync::atomic::Ordering::Relaxed);
    nxgfx::nvn::device::set_sdk_image(&img.bytes);
    let mut line = String::new();
    for w in 0..0xAAi32 {
        let mut v = 0i32;
        unsafe { nxgfx::nvn::device::nvnDeviceGetInteger(core::ptr::null(), w, &mut v) };
        line += &format!("{w:#x}={v} ");
    }
    println!("{line}");
}


/// The same frame's uniform-buffer descriptors written into a descriptor buffer (VK_EXT_descriptor_buffer,
/// vkGetDescriptorEXT) on a device of its own, against vkUpdateDescriptorSets: what descriptor buffers would save.
#[test]
#[ignore]
fn descriptor_buffer_writes_per_frame() {
    use std::ffi::CStr;
    let entry = unsafe { ash::Entry::load() }.unwrap();
    let ai = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
    let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&ai), None) }.unwrap();
    let pdev = unsafe { instance.enumerate_physical_devices() }
        .unwrap()
        .into_iter()
        .find(|&p| unsafe { instance.get_physical_device_properties(p) }.device_type == vk::PhysicalDeviceType::DISCRETE_GPU)
        .unwrap();
    let exts = [ash::ext::descriptor_buffer::NAME.as_ptr()];
    let mut f12 = vk::PhysicalDeviceVulkan12Features::default().buffer_device_address(true).descriptor_indexing(true).descriptor_binding_partially_bound(true);
    let mut fdb = vk::PhysicalDeviceDescriptorBufferFeaturesEXT::default().descriptor_buffer(true);
    let prio = [1.0f32];
    let qci = [vk::DeviceQueueCreateInfo::default().queue_family_index(0).queue_priorities(&prio)];
    let dev = unsafe { instance.create_device(pdev, &vk::DeviceCreateInfo::default().queue_create_infos(&qci).enabled_extension_names(&exts).push_next(&mut f12).push_next(&mut fdb), None) }.unwrap();
    let db = ash::ext::descriptor_buffer::Device::new(&instance, &dev);
    let mut dbp = vk::PhysicalDeviceDescriptorBufferPropertiesEXT::default();
    let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut dbp);
    unsafe { instance.get_physical_device_properties2(pdev, &mut p2) };
    let name = unsafe { CStr::from_ptr(p2.properties.device_name.as_ptr()) }.to_string_lossy().into_owned();
    // a 176-binding stage set as the replay's, with and without the descriptor-buffer flag
    let binds: Vec<_> = (0..176u32)
        .map(|b| {
            let ty = match b {
                0..=31 => vk::DescriptorType::UNIFORM_BUFFER,
                32..=63 => vk::DescriptorType::STORAGE_BUFFER,
                64..=127 => vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                128..=143 => vk::DescriptorType::STORAGE_IMAGE,
                144..=159 => vk::DescriptorType::UNIFORM_TEXEL_BUFFER,
                _ => vk::DescriptorType::STORAGE_TEXEL_BUFFER,
            };
            vk::DescriptorSetLayoutBinding::default().binding(b).descriptor_type(ty).descriptor_count(1).stage_flags(vk::ShaderStageFlags::ALL_GRAPHICS)
        })
        .collect();
    let dbl = unsafe { dev.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().flags(vk::DescriptorSetLayoutCreateFlags::DESCRIPTOR_BUFFER_EXT).bindings(&binds), None) }.unwrap();
    let set_size = unsafe { db.get_descriptor_set_layout_size(dbl) };
    let offs: Vec<u64> = (0..16u32).map(|b| unsafe { db.get_descriptor_set_layout_binding_offset(dbl, b) }).collect();
    // a buffer to point at, and host memory to write descriptors into (as a mapped descriptor buffer would be)
    let bci = vk::BufferCreateInfo::default().size(64 << 20).usage(vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS);
    let buf = unsafe { dev.create_buffer(&bci, None) }.unwrap();
    let req = unsafe { dev.get_buffer_memory_requirements(buf) };
    let mp = unsafe { instance.get_physical_device_memory_properties(pdev) };
    let ty = (0..mp.memory_type_count).find(|&i| req.memory_type_bits & (1 << i) != 0 && mp.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)).unwrap();
    let mut fl = vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
    let mem = unsafe { dev.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty).push_next(&mut fl), None) }.unwrap();
    unsafe { dev.bind_buffer_memory(buf, mem, 0) }.unwrap();
    let base = unsafe { dev.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buf)) };
    let n_sets = 2170usize;
    let mut host = vec![0u8; set_size as usize * n_sets];
    let dsz = dbp.uniform_buffer_descriptor_size;
    for round in 0..4 {
        let t0 = std::time::Instant::now();
        for i in 0..n_sets {
            for k in 0..15usize {
                let ai = vk::DescriptorAddressInfoEXT::default().address(base + ((i * 16 + k) as u64 * 256) % (60 << 20)).range(256);
                let gi = vk::DescriptorGetInfoEXT::default().ty(vk::DescriptorType::UNIFORM_BUFFER).data(vk::DescriptorDataEXT { p_uniform_buffer: &ai });
                let at = i * set_size as usize + offs[k] as usize;
                unsafe { db.get_descriptor(&gi, &mut host[at..at + dsz]) };
            }
        }
        println!("{name}: round {round}: {n_sets} sets x 15 uniform-buffer descriptors by vkGetDescriptorEXT in {:.2} ms (set {} bytes)", t0.elapsed().as_secs_f64() * 1e3, set_size);
    }
}

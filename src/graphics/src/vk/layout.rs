//! The one pipeline layout every graphics and compute program shares.
//!
//! Lane C's translator assigns bindings per shader (Ryujinx's numbering, described by its reflection json); at module
//! load `shader` rewrites each module's DescriptorSet / Binding decorations into this fixed layout, so programs never
//! need layouts of their own:
//!
//! - set s (0-5) = NVN shader stage s (vertex, fragment, geometry, tess control, tess evaluation, compute):
//!   - binding b (0-30) = constant bank c[b] (UNIFORM_BUFFER): c0 the driver segment, c1 the shader's constants, c2
//!     the handle table, c3 + r the game's uniform slot r, c24-c29 NVN2's push and stage banks;
//!   - binding 31 = the translator's support buffer (UNIFORM_BUFFER);
//!   - binding 32 + k = the module's k-th storage buffer (STORAGE_BUFFER, k < 32);
//!   - binding 64 + k = its k-th texture (COMBINED_IMAGE_SAMPLER, k < 64);
//!   - binding 128 + k = its k-th image (STORAGE_IMAGE, k < 16);
//!   - binding 144 + k / 160 + k = its k-th buffer texture / buffer image (UNIFORM / STORAGE_TEXEL_BUFFER, k < 16).
//! - set 6 = the pool set: binding 0 sampled images and 2 storage images by host texture ID, 1 samplers by host
//!   sampler ID, 3 / 4 texel buffers by texture ID (a shader that indexes NVN's pools directly; it aliases each array
//!   with the image types it needs).
//! - push constants (all stages, 128 bytes): 0 the active texture pool's base in the host table, 4 the sampler pool's,
//!   8 flags (bit 0: the viewport is flipped for an upper-left origin).
//! - NVN2's task and mesh shaders use sets 3 and 4 (a mesh pipeline has no tessellation stages).

use super::device::Device;
use ash::vk;

pub const STAGE_SETS: u32 = 6;
pub const POOL_SET: u32 = 6;
pub const BANKS: u32 = 31;
pub const SUPPORT: u32 = 31;
pub const SSBO_BASE: u32 = 32;
pub const SSBO_N: u32 = 32;
pub const TEX_BASE: u32 = 64;
pub const TEX_N: u32 = 64;
pub const IMG_BASE: u32 = 128;
pub const IMG_N: u32 = 16;
pub const TBUF_BASE: u32 = 144;
pub const IBUF_BASE: u32 = 160;
pub const XBUF_N: u32 = 16;
pub const STAGE_BINDINGS: u32 = 176;
pub const PUSH_SIZE: u32 = 128;
pub const PUSH_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::ALL_GRAPHICS.as_raw() | vk::ShaderStageFlags::COMPUTE.as_raw(),
);
pub const B_TEX: u32 = 0;
pub const B_SAMP: u32 = 1;
pub const B_IMG: u32 = 2;
pub const B_TBUF: u32 = 3;
pub const B_IBUF: u32 = 4;

pub fn stage_flag(s: u32) -> vk::ShaderStageFlags {
    match s {
        0 => vk::ShaderStageFlags::VERTEX,
        1 => vk::ShaderStageFlags::FRAGMENT,
        2 => vk::ShaderStageFlags::GEOMETRY,
        3 => vk::ShaderStageFlags::TESSELLATION_CONTROL,
        4 => vk::ShaderStageFlags::TESSELLATION_EVALUATION,
        _ => vk::ShaderStageFlags::COMPUTE,
    }
}

/// The stages a stage set's layout names. Every graphics stage set names every graphics stage (task and mesh
/// included when the device has them): NVN2's task and mesh shaders take the tessellation stages' sets 3 / 4, and with
/// those sets naming only TESSELLATION_CONTROL|TASK / TESSELLATION_EVALUATION|MESH, Mario Kart World's task/mesh
/// draws wrote storage binding 33 through set 0 (the vertex set) on NVIDIA's descriptor-buffer path: a stale
/// descriptor there named freed memory (the MKW device loss); with every set naming every stage the poison canary
/// finds no such write (MKW-RACE: 5-6 a run before, 0 after, `NXGFX_DB_FILL=0`). One stage mask for all graphics sets also keeps
/// every set's descriptor-buffer size and binding offsets those of set 0, which the writer uses for all of them.
pub fn stage_set_flags(s: u32, mesh: bool) -> vk::ShaderStageFlags {
    if s == 5 {
        return vk::ShaderStageFlags::COMPUTE;
    }
    if mesh {
        vk::ShaderStageFlags::ALL_GRAPHICS
            | vk::ShaderStageFlags::TASK_EXT
            | vk::ShaderStageFlags::MESH_EXT
    } else {
        vk::ShaderStageFlags::ALL_GRAPHICS
    }
}

/// The descriptor type of a binding of a stage set.
pub fn stage_binding_type(b: u32) -> vk::DescriptorType {
    match b {
        0..=31 => vk::DescriptorType::UNIFORM_BUFFER,
        32..=63 => vk::DescriptorType::STORAGE_BUFFER,
        64..=127 => vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        128..=143 => vk::DescriptorType::STORAGE_IMAGE,
        144..=159 => vk::DescriptorType::UNIFORM_TEXEL_BUFFER,
        _ => vk::DescriptorType::STORAGE_TEXEL_BUFFER,
    }
}

/// Descriptor counts by the kinds Vulkan limits (`maxPerStageDescriptor*` / `maxDescriptorSet*`, `maxPerStageResources`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub samplers: u64,
    pub uniform_buffers: u64,
    pub storage_buffers: u64,
    pub sampled_images: u64,
    pub storage_images: u64,
    pub resources: u64,
}

impl Counts {
    fn add(self, o: Counts) -> Counts {
        Counts {
            samplers: self.samplers + o.samplers,
            uniform_buffers: self.uniform_buffers + o.uniform_buffers,
            storage_buffers: self.storage_buffers + o.storage_buffers,
            sampled_images: self.sampled_images + o.sampled_images,
            storage_images: self.storage_images + o.storage_images,
            resources: self.resources + o.resources,
        }
    }
}

/// What `sets` stage sets hold, counted the way the limits count them: a combined image sampler is a sampler and a
/// sampled image, a uniform texel buffer a sampled image, a storage texel buffer a storage image; every one a resource.
pub fn stage_sets_count(sets: u64) -> Counts {
    let mut c = Counts::default();
    for b in 0..STAGE_BINDINGS {
        match stage_binding_type(b) {
            vk::DescriptorType::UNIFORM_BUFFER => c.uniform_buffers += 1,
            vk::DescriptorType::STORAGE_BUFFER => c.storage_buffers += 1,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER => {
                c.samplers += 1;
                c.sampled_images += 1;
            }
            vk::DescriptorType::UNIFORM_TEXEL_BUFFER => c.sampled_images += 1,
            _ => c.storage_images += 1,
        }
        c.resources += 1;
    }
    Counts {
        samplers: c.samplers * sets,
        uniform_buffers: c.uniform_buffers * sets,
        storage_buffers: c.storage_buffers * sets,
        sampled_images: c.sampled_images * sets,
        storage_images: c.storage_images * sets,
        resources: c.resources * sets,
    }
}

/// What the pool set holds: sampled images and uniform texel buffers, storage images and storage texel buffers by
/// texture ID, samplers by sampler ID.
pub fn pool_set_count(tex: u32, samp: u32) -> Counts {
    let t = tex as u64;
    Counts {
        samplers: samp as u64,
        uniform_buffers: 0,
        storage_buffers: 0,
        sampled_images: 2 * t,
        storage_images: 2 * t,
        resources: 4 * t,
    }
}

/// The device limits the shared layout must fit. `stage` / `set`: `maxPerStageDescriptor*` (+ `maxPerStageResources`)
/// and `maxDescriptorSet*`, which count the sets made without update-after-bind; `stage_uab` / `set_uab`: their
/// `UpdateAfterBind` forms, which count every set of a layout that has an update-after-bind set.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub stage: Counts,
    pub stage_uab: Counts,
    pub set: Counts,
    pub set_uab: Counts,
    pub bound_sets: u32,
    pub push_size: u32,
    pub colour_attachments: u32,
}

/// VK_EXT_descriptor_buffer's limits the layout relies on: two buffers bound at once (the pool set's and the draw's
/// chunk of stage sets), each a resource and a sampler descriptor buffer.
#[derive(Clone, Copy, Debug)]
pub struct DbLimits {
    pub bindings: u32,
    pub resource_bindings: u32,
    pub sampler_bindings: u32,
    pub resource_range: u64,
    pub sampler_range: u64,
    pub resource_space: u64,
    pub sampler_space: u64,
    pub space: u64,
}

const COUNT_NAMES: [&str; 6] = [
    "Samplers",
    "UniformBuffers",
    "StorageBuffers",
    "SampledImages",
    "StorageImages",
    "Resources",
];

fn fields(c: Counts) -> [u64; 6] {
    [
        c.samplers,
        c.uniform_buffers,
        c.storage_buffers,
        c.sampled_images,
        c.storage_images,
        c.resources,
    ]
}

/// Each way `need` exceeds `have`, as "<limit name>: needs N, the device allows M".
fn over(need: Counts, have: Counts, stage: bool, uab: bool, out: &mut Vec<String>) {
    for ((n, h), name) in fields(need).into_iter().zip(fields(have)).zip(COUNT_NAMES) {
        if n > h {
            let limit = match (stage, uab, name) {
                (true, false, "Resources") => "maxPerStageResources".to_string(),
                (true, true, "Resources") => "maxPerStageUpdateAfterBindResources".to_string(),
                (true, false, _) => format!("maxPerStageDescriptor{name}"),
                (true, true, _) => format!("maxPerStageDescriptorUpdateAfterBind{name}"),
                (false, false, _) => format!("maxDescriptorSet{name}"),
                (false, true, _) => format!("maxDescriptorSetUpdateAfterBind{name}"),
            };
            out.push(format!("{limit}: needs {n}, the device allows {h}"));
        }
    }
}

/// The pool set's texture and sampler capacities (`want_tex` / `want_samp`, lowered to what the limits leave once the
/// stage sets are counted), or every limit the shared layout breaks. A graphics stage sees five stage sets (every
/// graphics set names every graphics stage) and the pool set; a pipeline layout holds six stage sets and the pool
/// set; the fragment stage's colour attachments count as resources. With descriptor buffers no set is
/// update-after-bind, so everything counts against the plain limits; without them the stage sets count against the
/// plain limits and all sets together against the update-after-bind ones (the pool set is update-after-bind).
pub fn fit(
    l: &Limits,
    descbuf: bool,
    want_tex: u32,
    want_samp: u32,
) -> Result<(u32, u32), String> {
    let stage_fixed = stage_sets_count(5);
    let set_fixed = stage_sets_count(STAGE_SETS as u64);
    let colour = l.colour_attachments.min(8) as u64;
    let (stage_lim, set_lim) = if descbuf {
        (l.stage, l.set)
    } else {
        (l.stage_uab, l.set_uab)
    };
    let room = |have: u64, fixed: u64, per: u64| have.saturating_sub(fixed) / per;
    let tex = [
        room(stage_lim.sampled_images, stage_fixed.sampled_images, 2),
        room(stage_lim.storage_images, stage_fixed.storage_images, 2),
        room(stage_lim.resources, stage_fixed.resources + colour, 4),
        room(set_lim.sampled_images, set_fixed.sampled_images, 2),
        room(set_lim.storage_images, set_fixed.storage_images, 2),
    ]
    .into_iter()
    .fold(want_tex as u64, u64::min)
    .max(256) as u32;
    let samp = [
        room(stage_lim.samplers, stage_fixed.samplers, 1),
        room(set_lim.samplers, set_fixed.samplers, 1),
    ]
    .into_iter()
    .fold(want_samp as u64, u64::min)
    .max(64) as u32;
    let pool = pool_set_count(tex, samp);
    let attach = Counts {
        resources: colour,
        ..Counts::default()
    };
    let mut bad = Vec::new();
    if descbuf {
        over(stage_fixed.add(pool).add(attach), l.stage, true, false, &mut bad);
        over(set_fixed.add(pool), l.set, false, false, &mut bad);
    } else {
        over(stage_fixed.add(attach), l.stage, true, false, &mut bad);
        over(set_fixed, l.set, false, false, &mut bad);
        over(stage_fixed.add(pool).add(attach), l.stage_uab, true, true, &mut bad);
        over(set_fixed.add(pool), l.set_uab, false, true, &mut bad);
    }
    if l.bound_sets < STAGE_SETS + 1 {
        bad.push(format!(
            "maxBoundDescriptorSets: needs {}, the device allows {}",
            STAGE_SETS + 1,
            l.bound_sets
        ));
    }
    if l.push_size < PUSH_SIZE {
        bad.push(format!(
            "maxPushConstantsSize: needs {PUSH_SIZE}, the device allows {}",
            l.push_size
        ));
    }
    if bad.is_empty() {
        Ok((tex, samp))
    } else {
        Err(format!(
            "the shared pipeline layout (6 stage sets of {STAGE_BINDINGS} bindings, a pool set of {tex} textures / {samp} samplers) exceeds this device's descriptor limits: {}",
            bad.join("; ")
        ))
    }
}

/// Whether the descriptor buffers fit VK_EXT_descriptor_buffer's limits: two bound at once, the pool set's buffer
/// (`pool_size`) and a chunk of stage sets (`chunk`) each within a buffer's range, both within the address spaces.
pub fn db_fit(l: &DbLimits, pool_size: u64, chunk: u64) -> Result<(), String> {
    let mut bad = Vec::new();
    for (name, have) in [
        ("maxDescriptorBufferBindings", l.bindings),
        ("maxResourceDescriptorBufferBindings", l.resource_bindings),
        ("maxSamplerDescriptorBufferBindings", l.sampler_bindings),
    ] {
        if have < 2 {
            bad.push(format!("{name}: needs 2, the device allows {have}"));
        }
    }
    let big = pool_size.max(chunk);
    for (name, have) in [
        ("maxResourceDescriptorBufferRange", l.resource_range),
        ("maxSamplerDescriptorBufferRange", l.sampler_range),
    ] {
        if big > have {
            bad.push(format!("{name}: needs {big} bytes, the device allows {have}"));
        }
    }
    let both = pool_size + chunk;
    for (name, have) in [
        ("resourceDescriptorBufferAddressSpaceSize", l.resource_space),
        ("samplerDescriptorBufferAddressSpaceSize", l.sampler_space),
        ("descriptorBufferAddressSpaceSize", l.space),
    ] {
        if both > have {
            bad.push(format!("{name}: needs {both} bytes, the device allows {have}"));
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the descriptor buffers (a {} KB pool set, {} KB chunks of stage sets) exceed this device's limits: {}",
            pool_size >> 10,
            chunk >> 10,
            bad.join("; ")
        ))
    }
}

/// The device's limits for `fit`.
fn device_limits(d: &Device) -> Limits {
    let lim = &d.props.limits;
    let mut p12 = vk::PhysicalDeviceVulkan12Properties::default();
    {
        let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut p12);
        unsafe { d.instance.get_physical_device_properties2(d.pdev, &mut p2) };
    }
    let c = |samplers: u32, ubo: u32, ssbo: u32, sampled: u32, storage: u32, res: u32| Counts {
        samplers: samplers as u64,
        uniform_buffers: ubo as u64,
        storage_buffers: ssbo as u64,
        sampled_images: sampled as u64,
        storage_images: storage as u64,
        resources: res as u64,
    };
    Limits {
        stage: c(
            lim.max_per_stage_descriptor_samplers,
            lim.max_per_stage_descriptor_uniform_buffers,
            lim.max_per_stage_descriptor_storage_buffers,
            lim.max_per_stage_descriptor_sampled_images,
            lim.max_per_stage_descriptor_storage_images,
            lim.max_per_stage_resources,
        ),
        stage_uab: c(
            p12.max_per_stage_descriptor_update_after_bind_samplers,
            p12.max_per_stage_descriptor_update_after_bind_uniform_buffers,
            p12.max_per_stage_descriptor_update_after_bind_storage_buffers,
            p12.max_per_stage_descriptor_update_after_bind_sampled_images,
            p12.max_per_stage_descriptor_update_after_bind_storage_images,
            p12.max_per_stage_update_after_bind_resources,
        ),
        set: c(
            lim.max_descriptor_set_samplers,
            lim.max_descriptor_set_uniform_buffers,
            lim.max_descriptor_set_storage_buffers,
            lim.max_descriptor_set_sampled_images,
            lim.max_descriptor_set_storage_images,
            u32::MAX,
        ),
        set_uab: c(
            p12.max_descriptor_set_update_after_bind_samplers,
            p12.max_descriptor_set_update_after_bind_uniform_buffers,
            p12.max_descriptor_set_update_after_bind_storage_buffers,
            p12.max_descriptor_set_update_after_bind_sampled_images,
            p12.max_descriptor_set_update_after_bind_storage_images,
            u32::MAX,
        ),
        bound_sets: lim.max_bound_descriptor_sets,
        push_size: lim.max_push_constants_size,
        colour_attachments: lim.max_color_attachments,
    }
}

pub struct Layout {
    pub stage_sets: [vk::DescriptorSetLayout; STAGE_SETS as usize],
    /// descriptor buffers (`Device::caps.descbuf`): a stage set's size and its bindings' offsets in it, the pool set's
    /// size and its five bindings' offsets
    pub db_stage_size: u64,
    pub db_stage_off: Vec<u64>,
    pub db_pool_size: u64,
    pub db_pool_off: [u64; 5],
    pub pool_set: vk::DescriptorSetLayout,
    pub pipeline: vk::PipelineLayout,
    pub tex_cap: u32,
    pub samp_cap: u32,
    /// the push constant range's stages (with task and mesh when the device has them)
    pub push_stages: vk::ShaderStageFlags,
}

fn env_u32(n: &str, d: u32) -> u32 {
    std::env::var(n)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

impl Layout {
    pub fn new(d: &Device) -> Result<Layout, String> {
        let dev = &d.dev;
        // Mario Kart World registers texture IDs up to ~131,400 (its texture pool's; 65,536 left the rest unbound).
        // Checked against the device's limits before any layout is made: a layout past them is undefined behaviour
        // (fine on NVIDIA's 1M-per-stage limits, unchecked elsewhere), so the device is refused with the limits named.
        let (tex_cap, samp_cap) = fit(
            &device_limits(d),
            d.caps.descbuf,
            env_u32("NXGFX_TEX_CAP", 262144),
            env_u32("NXGFX_SAMP_CAP", 16384),
        )?;
        let mut stage_sets = [vk::DescriptorSetLayout::null(); STAGE_SETS as usize];
        for (s, slot) in stage_sets.iter_mut().enumerate() {
            let sf = stage_set_flags(s as u32, d.caps.mesh);
            let binds: Vec<_> = (0..STAGE_BINDINGS)
                .map(|b| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(b)
                        .descriptor_type(stage_binding_type(b))
                        .descriptor_count(1)
                        .stage_flags(sf)
                })
                .collect();
            let flags = vec![vk::DescriptorBindingFlags::PARTIALLY_BOUND; binds.len()];
            let mut bf =
                vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
            let lf = if d.caps.descbuf {
                vk::DescriptorSetLayoutCreateFlags::DESCRIPTOR_BUFFER_EXT
            } else {
                vk::DescriptorSetLayoutCreateFlags::empty()
            };
            let ci = vk::DescriptorSetLayoutCreateInfo::default()
                .flags(lf)
                .bindings(&binds)
                .push_next(&mut bf);
            *slot = unsafe { dev.create_descriptor_set_layout(&ci, None) }
                .map_err(|e| format!("stage set layout: {e}"))?;
        }
        let all = vk::ShaderStageFlags::ALL;
        let binds = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(B_TEX)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(tex_cap)
                .stage_flags(all),
            vk::DescriptorSetLayoutBinding::default()
                .binding(B_SAMP)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(samp_cap)
                .stage_flags(all),
            vk::DescriptorSetLayoutBinding::default()
                .binding(B_IMG)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(tex_cap)
                .stage_flags(all),
            vk::DescriptorSetLayoutBinding::default()
                .binding(B_TBUF)
                .descriptor_type(vk::DescriptorType::UNIFORM_TEXEL_BUFFER)
                .descriptor_count(tex_cap)
                .stage_flags(all),
            vk::DescriptorSetLayoutBinding::default()
                .binding(B_IBUF)
                .descriptor_type(vk::DescriptorType::STORAGE_TEXEL_BUFFER)
                .descriptor_count(tex_cap)
                .stage_flags(all),
        ];
        // descriptor buffers: written in place in the buffer (no update-after-bind: the buffer is the descriptors)
        let f = if d.caps.descbuf {
            vk::DescriptorBindingFlags::PARTIALLY_BOUND
        } else {
            vk::DescriptorBindingFlags::PARTIALLY_BOUND
                | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING
        };
        let flags = [f; 5];
        let mut bf = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
        let lf = if d.caps.descbuf {
            vk::DescriptorSetLayoutCreateFlags::DESCRIPTOR_BUFFER_EXT
        } else {
            vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL
        };
        let ci = vk::DescriptorSetLayoutCreateInfo::default()
            .flags(lf)
            .bindings(&binds)
            .push_next(&mut bf);
        let pool_set = unsafe { dev.create_descriptor_set_layout(&ci, None) }
            .map_err(|e| format!("pool set layout: {e}"))?;
        let mut sets = stage_sets.to_vec();
        sets.push(pool_set);
        let push_stages = if d.caps.mesh {
            PUSH_STAGES | vk::ShaderStageFlags::TASK_EXT | vk::ShaderStageFlags::MESH_EXT
        } else {
            PUSH_STAGES
        };
        let pcr = [vk::PushConstantRange::default()
            .stage_flags(push_stages)
            .offset(0)
            .size(PUSH_SIZE)];
        let ci = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&sets)
            .push_constant_ranges(&pcr);
        let pipeline = unsafe { dev.create_pipeline_layout(&ci, None) }
            .map_err(|e| format!("pipeline layout: {e}"))?;
        let (mut db_stage_size, mut db_stage_off, mut db_pool_size, mut db_pool_off) =
            (0, Vec::new(), 0, [0u64; 5]);
        if d.caps.descbuf {
            let al = d.db_props.descriptor_buffer_offset_alignment.max(1);
            db_stage_size =
                unsafe { d.db.get_descriptor_set_layout_size(stage_sets[0]) }.next_multiple_of(al);
            db_stage_off = (0..STAGE_BINDINGS)
                .map(|b| unsafe {
                    d.db.get_descriptor_set_layout_binding_offset(stage_sets[0], b)
                })
                .collect();
            // the writer places every stage set by set 0's offsets: say so when a set's own layout differs
            for (s, &l) in stage_sets.iter().enumerate().skip(1) {
                let size = unsafe { d.db.get_descriptor_set_layout_size(l) }.next_multiple_of(al);
                let off = (0..STAGE_BINDINGS).find(|&b| {
                    let o = unsafe { d.db.get_descriptor_set_layout_binding_offset(l, b) };
                    o != db_stage_off[b as usize]
                });
                if size != db_stage_size || off.is_some() {
                    crate::vklog!("[RED] descriptor buffers: stage set {s}'s layout differs from set 0's (size {size} vs {db_stage_size}, first other binding offset {off:?})");
                }
            }
            db_pool_size = unsafe { d.db.get_descriptor_set_layout_size(pool_set) };
            for (i, o) in db_pool_off.iter_mut().enumerate() {
                *o = unsafe {
                    d.db.get_descriptor_set_layout_binding_offset(pool_set, i as u32)
                };
            }
            let p = &d.db_props;
            db_fit(
                &DbLimits {
                    bindings: p.max_descriptor_buffer_bindings,
                    resource_bindings: p.max_resource_descriptor_buffer_bindings,
                    sampler_bindings: p.max_sampler_descriptor_buffer_bindings,
                    resource_range: p.max_resource_descriptor_buffer_range,
                    sampler_range: p.max_sampler_descriptor_buffer_range,
                    resource_space: p.resource_descriptor_buffer_address_space_size,
                    sampler_space: p.sampler_descriptor_buffer_address_space_size,
                    space: p.descriptor_buffer_address_space_size,
                },
                db_pool_size,
                super::queue::DB_CHUNK.max(db_stage_size * 2),
            )?;
        }
        crate::vklog!(
            "graphics options: driver_exclusive {} ({}), shader_thread {}, low_priority_shader_compiler {}, fps_target {} (frame skip {})",
            nxconfig::string("driver_exclusive"),
            if super::pipe::driver_exclusive_d(d) { "pipeline creation and drawing never at once" } else { "off" },
            nxconfig::flag("shader_thread") as u8,
            nxconfig::flag("low_priority_shader_compiler") as u8,
            nxconfig::string("fps_target"),
            nxconfig::frame_skip()
        );
        crate::vklog!(
            "if the GPU stops responding: checkpoints {} (NXGFX_CHECKPOINTS), the driver's fault report {}, loops bounded at {} back-edges a call (NXGFX_LOOP_GUARD); a lost device ends the game with exit code 71",
            if !d.caps.checkpoints { "off" } else if super::lost::level() >= 2 { "before every draw and dispatch" } else { "before dispatches and indirect draws" },
            if d.caps.device_fault { "on" } else { "not available" },
            super::shader::loop_cap()
        );
        crate::vklog!(
            "shared pipeline layout: 6 stage sets of {STAGE_BINDINGS} bindings, pool set of {tex_cap} textures / {samp_cap} samplers{}",
            if d.caps.descbuf { format!("; descriptor buffers: a stage set {db_stage_size} bytes, the pool set {} KB", db_pool_size >> 10) } else { String::new() }
        );
        Ok(Layout {
            stage_sets,
            pool_set,
            pipeline,
            tex_cap,
            samp_cap,
            push_stages,
            db_stage_size,
            db_stage_off,
            db_pool_size,
            db_pool_off,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vk::ShaderStageFlags as F;

    /// Every graphics stage set names every graphics stage, task and mesh included (a task shader in set 3 and a
    /// mesh shader in set 4 reached set 0's descriptors when those sets named only their own stages), and all of them
    /// one mask, so the descriptor-buffer layout of set 0 (sizes, binding offsets) holds for each.
    #[test]
    fn graphics_stage_sets_name_every_graphics_stage() {
        for mesh in [false, true] {
            let first = stage_set_flags(0, mesh);
            for s in 0..5 {
                let f = stage_set_flags(s, mesh);
                assert_eq!(f, first, "set {s} (mesh {mesh})");
                assert!(f.contains(F::ALL_GRAPHICS), "set {s}: {f:?}");
                assert_eq!(f.contains(F::TASK_EXT | F::MESH_EXT), mesh, "set {s}: {f:?}");
                assert!(f.contains(stage_flag(s)), "set {s} names its own stage");
            }
            assert_eq!(stage_set_flags(5, mesh), F::COMPUTE);
        }
        // the task and mesh shaders' own sets (3 / 4) name them
        assert!(stage_set_flags(3, true).contains(F::TASK_EXT));
        assert!(stage_set_flags(4, true).contains(F::MESH_EXT));
        // and the vertex set does too: a set 0 a task/mesh draw reaches is one its layout allows
        assert!(stage_set_flags(0, true).contains(F::TASK_EXT | F::MESH_EXT));
    }

    fn all(n: u64) -> Counts {
        Counts {
            samplers: n,
            uniform_buffers: n,
            storage_buffers: n,
            sampled_images: n,
            storage_images: n,
            resources: n,
        }
    }

    /// Per-set limits: Vulkan has no per-set resource limit (`device_limits` puts u32::MAX there).
    fn set_of(n: u64) -> Counts {
        Counts {
            resources: u32::MAX as u64,
            ..all(n)
        }
    }

    /// The RTX 5090's limits (vulkaninfo, this machine): 1,048,576 per stage, resources u32::MAX.
    fn nvidia() -> Limits {
        let mut stage = all(1 << 20);
        stage.resources = u32::MAX as u64;
        Limits {
            stage,
            stage_uab: stage,
            set: set_of(1 << 20),
            set_uab: set_of(1 << 20),
            bound_sets: 32,
            push_size: 256,
            colour_attachments: 8,
        }
    }

    /// An older Intel / mobile-class set: small plain limits (update-after-bind ones large).
    fn strict() -> Limits {
        Limits {
            stage: Counts {
                samplers: 64,
                uniform_buffers: 200,
                storage_buffers: 200,
                sampled_images: 200,
                storage_images: 16,
                resources: 200,
            },
            stage_uab: all(1 << 20),
            set: set_of(1024),
            set_uab: set_of(1 << 20),
            bound_sets: 8,
            push_size: 128,
            colour_attachments: 8,
        }
    }

    /// One graphics stage sees five stage sets: 880 descriptors (the ledger's ~880).
    #[test]
    fn five_stage_sets_count_as_the_limits_do() {
        let c = stage_sets_count(5);
        assert_eq!(c.uniform_buffers, 160);
        assert_eq!(c.storage_buffers, 160);
        assert_eq!(c.samplers, 320);
        assert_eq!(c.sampled_images, 400, "combined image samplers and uniform texel buffers");
        assert_eq!(c.storage_images, 160, "storage images and storage texel buffers");
        assert_eq!(c.resources, 880);
    }

    /// NVIDIA keeps the full pool set with and without descriptor buffers.
    #[test]
    fn nvidia_fits_the_whole_pool_set() {
        assert_eq!(fit(&nvidia(), true, 262144, 16384), Ok((262144, 16384)));
        assert_eq!(fit(&nvidia(), false, 262144, 16384), Ok((262144, 16384)));
    }

    /// A tighter set (half NVIDIA's per-stage counts, resources under a million): the pool set is sized down to what
    /// the stage sets leave, never past a limit. The old rule (half the sampled-image limit) ignored the stage sets' 400 sampled
    /// images and the 4 resources per texture ID.
    #[test]
    fn the_pool_set_is_sized_from_the_limits_the_stage_sets_leave() {
        let mut l = nvidia();
        l.stage = all(524_288);
        l.stage.resources = 1_000_000;
        let (tex, samp) = fit(&l, true, 262144, 16384).unwrap();
        assert_eq!(tex, (1_000_000 - 880 - 8) / 4, "resources bound it");
        assert_eq!(samp, 16384);
        let need = stage_sets_count(5).add(pool_set_count(tex, samp));
        assert!(need.sampled_images <= 524_288 && need.resources + 8 <= 1_000_000);
        // an explicit NXGFX_TEX_CAP below the room stays
        assert_eq!(fit(&l, true, 4096, 1024), Ok((4096, 1024)));
    }

    /// A device whose plain limits cannot hold even the stage sets is refused, naming each broken limit.
    #[test]
    fn a_strict_device_is_refused_with_the_limits_named() {
        let e = fit(&strict(), false, 262144, 16384).unwrap_err();
        assert!(e.contains("maxPerStageDescriptorStorageImages: needs 160, the device allows 16"), "{e}");
        assert!(e.contains("maxPerStageDescriptorSamplers: needs 320"), "{e}");
        assert!(e.contains("maxPerStageResources: needs 888"), "{e}");
        assert!(!e.contains("UpdateAfterBind"), "{e}");
        // with descriptor buffers the pool set counts against the plain limits too
        let e = fit(&strict(), true, 262144, 16384).unwrap_err();
        assert!(e.contains("maxPerStageDescriptorSampledImages"), "{e}");
        let mut l = nvidia();
        l.bound_sets = 4;
        assert!(fit(&l, true, 262144, 16384).unwrap_err().contains("maxBoundDescriptorSets: needs 7"));
    }

    /// Descriptor buffers: two bound at once, the pool set's buffer within the range and address spaces.
    #[test]
    fn descriptor_buffer_limits_are_checked() {
        let big = DbLimits {
            bindings: 32,
            resource_bindings: 32,
            sampler_bindings: 32,
            resource_range: 1 << 40,
            sampler_range: 1 << 40,
            resource_space: 1 << 40,
            sampler_space: 1 << 40,
            space: 1 << 40,
        };
        assert_eq!(db_fit(&big, 10304 << 10, 4 << 20), Ok(()));
        let small = DbLimits {
            sampler_bindings: 1,
            sampler_range: 8 << 20,
            sampler_space: 12 << 20,
            ..big
        };
        let e = db_fit(&small, 10304 << 10, 4 << 20).unwrap_err();
        assert!(e.contains("maxSamplerDescriptorBufferBindings: needs 2"), "{e}");
        assert!(e.contains("maxSamplerDescriptorBufferRange"), "{e}");
        assert!(e.contains("samplerDescriptorBufferAddressSpaceSize"), "{e}");
        assert!(!e.contains("maxResource"), "{e}");
    }
}

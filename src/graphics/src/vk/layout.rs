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
pub const PUSH_STAGES: vk::ShaderStageFlags =
    vk::ShaderStageFlags::from_raw(vk::ShaderStageFlags::ALL_GRAPHICS.as_raw() | vk::ShaderStageFlags::COMPUTE.as_raw());
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

pub struct Layout {
    pub stage_sets: [vk::DescriptorSetLayout; STAGE_SETS as usize],
    pub pool_set: vk::DescriptorSetLayout,
    pub pipeline: vk::PipelineLayout,
    pub tex_cap: u32,
    pub samp_cap: u32,
}

fn env_u32(n: &str, d: u32) -> u32 {
    std::env::var(n).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

impl Layout {
    pub fn new(d: &Device) -> Result<Layout, String> {
        let dev = &d.dev;
        let mut stage_sets = [vk::DescriptorSetLayout::null(); STAGE_SETS as usize];
        for (s, slot) in stage_sets.iter_mut().enumerate() {
            let sf = stage_flag(s as u32);
            let binds: Vec<_> = (0..STAGE_BINDINGS)
                .map(|b| vk::DescriptorSetLayoutBinding::default().binding(b).descriptor_type(stage_binding_type(b)).descriptor_count(1).stage_flags(sf))
                .collect();
            let flags = vec![vk::DescriptorBindingFlags::PARTIALLY_BOUND; binds.len()];
            let mut bf = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
            let ci = vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds).push_next(&mut bf);
            *slot = unsafe { dev.create_descriptor_set_layout(&ci, None) }.map_err(|e| format!("stage set layout: {e}"))?;
        }
        let lim = &d.props.limits;
        let tex_cap = env_u32("NXGFX_TEX_CAP", 65536).min(lim.max_per_stage_descriptor_sampled_images / 2).max(256);
        let samp_cap = env_u32("NXGFX_SAMP_CAP", 4096).min(lim.max_per_stage_descriptor_samplers / 2).max(64);
        let all = vk::ShaderStageFlags::ALL;
        let binds = [
            vk::DescriptorSetLayoutBinding::default().binding(B_TEX).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(tex_cap).stage_flags(all),
            vk::DescriptorSetLayoutBinding::default().binding(B_SAMP).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(samp_cap).stage_flags(all),
            vk::DescriptorSetLayoutBinding::default().binding(B_IMG).descriptor_type(vk::DescriptorType::STORAGE_IMAGE).descriptor_count(tex_cap).stage_flags(all),
            vk::DescriptorSetLayoutBinding::default().binding(B_TBUF).descriptor_type(vk::DescriptorType::UNIFORM_TEXEL_BUFFER).descriptor_count(tex_cap).stage_flags(all),
            vk::DescriptorSetLayoutBinding::default().binding(B_IBUF).descriptor_type(vk::DescriptorType::STORAGE_TEXEL_BUFFER).descriptor_count(tex_cap).stage_flags(all),
        ];
        let f = vk::DescriptorBindingFlags::PARTIALLY_BOUND | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING;
        let flags = [f; 5];
        let mut bf = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
        let ci = vk::DescriptorSetLayoutCreateInfo::default().flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL).bindings(&binds).push_next(&mut bf);
        let pool_set = unsafe { dev.create_descriptor_set_layout(&ci, None) }.map_err(|e| format!("pool set layout: {e}"))?;
        let mut sets = stage_sets.to_vec();
        sets.push(pool_set);
        let pcr = [vk::PushConstantRange::default().stage_flags(PUSH_STAGES).offset(0).size(PUSH_SIZE)];
        let ci = vk::PipelineLayoutCreateInfo::default().set_layouts(&sets).push_constant_ranges(&pcr);
        let pipeline = unsafe { dev.create_pipeline_layout(&ci, None) }.map_err(|e| format!("pipeline layout: {e}"))?;
        crate::vklog!("shared pipeline layout: 6 stage sets of {STAGE_BINDINGS} bindings, pool set of {tex_cap} textures / {samp_cap} samplers");
        Ok(Layout { stage_sets, pool_set, pipeline, tex_cap, samp_cap })
    }
}

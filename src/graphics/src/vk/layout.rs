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
        let lim = &d.props.limits;
        // Mario Kart World registers texture IDs up to ~131,400 (its texture pool's; 65,536 left the rest unbound)
        let tex_cap = env_u32("NXGFX_TEX_CAP", 262144)
            .min(lim.max_per_stage_descriptor_sampled_images / 2)
            .max(256);
        let samp_cap = env_u32("NXGFX_SAMP_CAP", 16384)
            .min(lim.max_per_stage_descriptor_samplers / 2)
            .max(64);
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
}

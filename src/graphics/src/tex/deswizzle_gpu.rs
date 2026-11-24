//! Block-linear -> linear on the GPU (nxnative `nvn_deswizzle_gpu`): a texture uploaded from its storage is staged as
//! it is stored and deswizzled by a compute pass (`shaders/deswizzle.comp`) into rows the image copy reads, instead of
//! on the CPU (`deswizzle.rs`, which stays the reference and the path for everything this one does not take).
//!
//! Taken for block-linear textures (not linear, multisample, buffer, ASTC or 3-byte-expanded formats) of at least
//! `MIN_BYTES` whose rows are whole 32-bit words. `NXGFX_GPU_DESWIZZLE=0` off.

use super::deswizzle::{geom, GOB_SWITCH2};
use super::layout::{level_gobs, TexDesc};
use crate::vk::queue::Sub;
use crate::vk::Gpu;
use ash::vk;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

static SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/deswizzle_comp.spv"));
/// Textures smaller than this are deswizzled on the CPU (a dispatch and its barrier cost more than the copy).
pub const MIN_BYTES: u64 = 256 << 10;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Push {
    base: u32,
    row_bytes: u32,
    rows: u32,
    slices: u32,
    th: u32,
    td: u32,
    gobs_x: u32,
    pitch: u32,
    switch2: u32,
}

/// (set layout, pipeline layout, pipeline) as raw handles.
static PIPE: OnceLock<Option<(u64, u64, u64)>> = OnceLock::new();

fn pipe(g: &Gpu) -> Option<(vk::DescriptorSetLayout, vk::PipelineLayout, vk::Pipeline)> {
    use vk::Handle;
    let p = PIPE.get_or_init(|| {
        let dev = g.dev();
        let binds: Vec<_> = (0..2)
            .map(|i| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(i)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            })
            .collect();
        let sl = unsafe {
            dev.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds),
                None,
            )
        }
        .ok()?;
        let pcr = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .size(core::mem::size_of::<Push>() as u32)];
        let sls = [sl];
        let pl = unsafe {
            dev.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sls)
                    .push_constant_ranges(&pcr),
                None,
            )
        }
        .ok()?;
        let w: Vec<u32> = SPV
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let m = unsafe {
            dev.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&w), None)
        }
        .ok()?;
        let st = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(m)
            .name(c"main");
        let p = unsafe {
            dev.create_compute_pipelines(
                g.pipes.cache,
                &[vk::ComputePipelineCreateInfo::default()
                    .stage(st)
                    .layout(pl)],
                None,
            )
        }
        .map(|v| v[0])
        .ok();
        unsafe { dev.destroy_shader_module(m, None) };
        crate::vklog!(
            "block-linear textures are deswizzled on the GPU{}",
            if p.is_some() {
                ""
            } else {
                ": its pipeline failed, on the CPU instead"
            }
        );
        Some((sl.as_raw(), pl.as_raw(), p?.as_raw()))
    });
    p.map(|(a, b, c)| {
        (
            vk::DescriptorSetLayout::from_raw(a),
            vk::PipelineLayout::from_raw(b),
            vk::Pipeline::from_raw(c),
        )
    })
}

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_GPU_DESWIZZLE").as_deref() != Ok("0"))
}

/// Whether every level and layer of `d` can take the GPU path (whole 32-bit words, block-linear).
pub fn fits(d: &TexDesc, layers: u32, levels: u32) -> bool {
    if !on() {
        return false;
    }
    for layer in 0..layers {
        for level in 0..levels {
            let Some(g) = geom(d, level, layer) else {
                return false;
            };
            if g.row_bytes % 4 != 0 || g.base % 4 != 0 {
                return false;
            }
        }
    }
    true
}

/// Records the deswizzle of one level and layer of `storage` (staged at `src` / `src_off`, the whole texture) into
/// rows of its block-row bytes in device scratch: (buffer, offset, bytes), or None when it does not apply.
pub fn record(
    g: &Gpu,
    sub: &mut Sub,
    d: &TexDesc,
    level: u32,
    layer: u32,
    src: vk::Buffer,
    src_off: u64,
    src_len: u64,
) -> Option<(vk::Buffer, u64, u64)> {
    let gm = geom(d, level, layer)?;
    if gm.row_bytes % 4 != 0 || gm.base % 4 != 0 {
        return None;
    }
    let lb = (level_gobs(d, level) * 64).max(gm.row_bytes);
    let n = gm.row_bytes as u64 * gm.rows as u64 * gm.slices as u64;
    let (sl, pl, p) = pipe(g)?;
    let (dbuf, doff) = sub.gpu_scratch(g, n.max(4))?;
    let set = sub.alloc_set(g, sl);
    let bi = [
        vk::DescriptorBufferInfo {
            buffer: src,
            offset: src_off,
            range: src_len.max(4),
        },
        vk::DescriptorBufferInfo {
            buffer: dbuf,
            offset: doff,
            range: n.max(4),
        },
    ];
    let wr: Vec<_> = (0..2)
        .map(|i| {
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(i as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&bi[i..i + 1])
        })
        .collect();
    let pc = Push {
        base: gm.base as u32,
        row_bytes: gm.row_bytes,
        rows: gm.rows,
        slices: gm.slices,
        th: gm.th,
        td: gm.td,
        gobs_x: lb.div_ceil(64),
        pitch: gm.row_bytes,
        switch2: GOB_SWITCH2.load(Ordering::Relaxed) as u32,
    };
    let dev = g.dev();
    crate::vk::lost::checkpoint(g, sub.pre, false, || {
        format!(
            "a deswizzle of {}x{} format {:#x} level {level} layer {layer}",
            d.width, d.height, d.format
        )
    });
    unsafe {
        dev.update_descriptor_sets(&wr, &[]);
        dev.cmd_bind_pipeline(sub.pre, vk::PipelineBindPoint::COMPUTE, p);
        dev.cmd_bind_descriptor_sets(sub.pre, vk::PipelineBindPoint::COMPUTE, pl, 0, &[set], &[]);
        dev.cmd_push_constants(
            sub.pre,
            pl,
            vk::ShaderStageFlags::COMPUTE,
            0,
            core::slice::from_raw_parts(
                &pc as *const Push as *const u8,
                core::mem::size_of::<Push>(),
            ),
        );
        dev.cmd_dispatch(sub.pre, (gm.row_bytes / 4).div_ceil(64), gm.rows, gm.slices);
    }
    crate::vk::queue::full_barrier(g, sub.pre);
    Some((dbuf, doff, n))
}

/// Deswizzles one level and layer of `storage` on the GPU and reads it back (tests): rows of its block-row bytes.
pub fn deswizzle_on_gpu(
    g: &'static Gpu,
    d: &TexDesc,
    level: u32,
    layer: u32,
    storage: &[u8],
) -> Option<Vec<u8>> {
    let gm = geom(d, level, layer)?;
    let n = gm.row_bytes as u64 * gm.rows as u64 * gm.slices as u64;
    let rb = g
        .buffer(
            n.max(4),
            vk::BufferUsageFlags::TRANSFER_DST,
            gpu_allocator::MemoryLocation::GpuToCpu,
            "deswizzle readback",
        )
        .ok()?;
    let mut ok = false;
    let f = crate::vk::queue::oneoff(g, |g, sub, _| {
        let (sb, so) = sub.stage_bytes_fresh(g, storage);
        if let Some((b, o, n)) = record(g, sub, d, level, layer, sb, so, storage.len() as u64) {
            unsafe {
                g.dev().cmd_copy_buffer(
                    sub.pre,
                    b,
                    rb.buf,
                    &[vk::BufferCopy {
                        src_offset: o,
                        dst_offset: 0,
                        size: n,
                    }],
                )
            };
            ok = true;
        }
    });
    crate::vk::queue::wait_fence(f, u64::MAX);
    let out = ok.then(|| unsafe { core::slice::from_raw_parts(rb.ptr, n as usize) }.to_vec());
    g.free_buffer(rb);
    out
}

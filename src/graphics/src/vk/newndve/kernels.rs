//! Bounded scalar compute kernels for NDVE arithmetic certification. These use a
//! canonical buffer ABI, not the game's tensor tiling. They cannot consume a guest
//! dispatch. Pipeline creation and all bounds checks precede command recording.

use super::Kernel;
use crate::vk::{queue::Sub, Gpu};
use ash::vk;

#[cfg(test)]
#[path = "planner_tests.rs"]
mod planner_tests;

const LAYER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/ndve_layer_comp.spv"));
const RESAMPLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/ndve_resample_comp.spv"));
const OUT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/ndve_out_comp.spv"));
const RESAMPLE_IMAGE: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/spv/ndve_resample_image_comp.spv"
));

#[derive(Clone, Copy, Debug)]
pub struct Job {
    pub kernel: Kernel,
    /// Output size for ordinary/down convolution, input size for up and out.
    pub width: u32,
    pub height: u32,
    pub dither: [u32; 2],
    pub flip: bool,
    rounding: u32,
    image_compare: bool,
}

impl Job {
    pub fn new(kernel: Kernel, width: u32, height: u32) -> Self {
        Self {
            kernel,
            width,
            height,
            dither: [512, 512],
            flip: false,
            rounding: 16,
            image_compare: false,
        }
    }

    fn compare_rounding(mut self) -> Self {
        if std::env::var("NXGFX_NDVE_COMPARE_8").as_deref() == Ok("1") {
            self.rounding = 8;
        }
        self
    }

    fn image_compare(mut self, rgba8: bool) -> Self {
        self.image_compare = true;
        self.flip = rgba8;
        self
    }

    /// Byte extents of input, output, weights, bias/dither, skip/picture.
    pub fn extents(self) -> Result<[u64; 5], String> {
        let Self {
            kernel,
            width: w,
            height: h,
            ..
        } = self;
        // Includes production dimensions, while excluding arithmetic overflow and
        // accidentally enormous synthetic submissions.
        if w == 0 || h == 0 || w > 2560 || h > 1440 {
            return Err("NDVE dimensions".into());
        }
        // The native graph reserves sixteen zero rows after the bordered tensor so
        // tiled last-row loads remain in allocation while writers touch only interiors.
        let tensor =
            |w: u32, h: u32, c: u32| u64::from(w + 2) * u64::from(h + 2 + 16) * u64::from(c) * 2;
        let pixels = u64::from(w) * u64::from(h);
        let [t, k, n] = kernel.shape();
        Ok(match kernel {
            Kernel::Pack => [pixels * 16, tensor(w, h, 16), 16, 16, 16],
            Kernel::Conv16 | Kernel::Conv32 | Kernel::Conv64 => [
                tensor(w, h, k),
                tensor(w, h, n),
                u64::from(t * k * n * 2),
                u64::from(n * 2),
                16,
            ],
            Kernel::Down16 | Kernel::Down32 => [
                tensor(2 * w, 2 * h, k),
                tensor(w, h, n),
                u64::from(t * k * n * 2),
                u64::from(n * 2),
                16,
            ],
            Kernel::Up64 | Kernel::Up32 => [
                tensor(w, h, k),
                tensor(2 * w, 2 * h, n),
                u64::from(t * k * n * 2),
                u64::from(n * 2),
                tensor(2 * w, 2 * h, n),
            ],
            Kernel::Out => [
                tensor(w, h, 16),
                pixels * 16,
                9 * 16 * 16 * 2,
                32,
                pixels * 16,
            ],
            Kernel::Luma if pixels % 2 == 0 => [pixels * 4, pixels * 2, 16, 16, 16],
            Kernel::Resample
                if w % 6 == 0 && h % 6 == 0 && self.dither.iter().all(|&d| d > 0 && d <= 512) =>
            {
                [
                    pixels * 2,
                    u64::from(w * 4 / 3) * u64::from(h * 4 / 3) * 4,
                    0x3000,
                    u64::from(self.dither[0]) * u64::from(self.dither[1]) * 4,
                    pixels * 4,
                ]
            }
            Kernel::Compare => [pixels * 4, 8, 16, 16, pixels * 4],
            _ => return Err("NDVE luma/resample alignment or dither size".into()),
        })
    }

    pub(super) fn args(self) -> [u32; 8] {
        let [_, k, n] = self.kernel.shape();
        let pixels = self.width * self.height;
        let (kind, count) = match self.kernel {
            Kernel::Pack => (0, pixels * 8),
            Kernel::Conv16 | Kernel::Conv32 | Kernel::Conv64 => (1, pixels * n / 2),
            Kernel::Down16 | Kernel::Down32 => (2, pixels * n / 2),
            Kernel::Up64 | Kernel::Up32 => (3, pixels * 4 * n / 2),
            Kernel::Out => (4, pixels * 4),
            Kernel::Luma => (5, pixels / 2),
            Kernel::Resample => (6, (self.width * 4 / 3) * (self.height * 4 / 3)),
            Kernel::Compare if self.image_compare => (8, pixels),
            Kernel::Compare => (7, pixels),
        };
        let (k, n) = if self.kernel == Kernel::Resample {
            (self.dither[0], self.dither[1])
        } else {
            (k, n)
        };
        // Retained dmsh_hmma16816_f16: C, 16 ordered FP32 FMAs, one half pack.
        // This is translated Vulkan compatibility, not a console/Metal claim.
        [
            self.width,
            self.height,
            k,
            n,
            kind,
            count,
            self.rounding,
            self.flip as u32,
        ]
    }
}

pub struct Kernels<'g> {
    g: &'g Gpu,
    set: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipes: [vk::Pipeline; 4],
    sampler: vk::Sampler,
    limits: vk::PhysicalDeviceLimits,
    hashes: [u128; 4],
}

pub(crate) struct PreparedDispatch {
    set: vk::DescriptorSet,
    pipe: vk::Pipeline,
    args: [u32; 8],
    evidence: super::trace::Record,
}

pub(crate) struct PreparedGraph {
    kernels: &'static Kernels<'static>,
    zero: Vec<vk::DescriptorBufferInfo>,
    dispatches: Vec<PreparedDispatch>,
}

impl PreparedGraph {
    pub(crate) fn record(self, g: &Gpu, sub: &mut Sub) {
        for b in self.zero {
            unsafe { g.dev().cmd_fill_buffer(sub.main, b.buffer, b.offset, b.range, 0) };
        }
        crate::vk::queue::full_barrier(g, sub.main);
        for dispatch in self.dispatches { self.kernels.issue(sub, dispatch); }
    }
}

#[derive(Clone, Copy)]
pub struct GraphBuffers {
    pub picture: vk::DescriptorBufferInfo,
    pub network_output: vk::DescriptorBufferInfo,
    pub final_output: vk::DescriptorBufferInfo,
    pub dither: vk::DescriptorBufferInfo,
    pub weights: [vk::DescriptorBufferInfo; 12],
    pub bias: [vk::DescriptorBufferInfo; 12],
    pub resample_weights: vk::DescriptorBufferInfo,
    pub picture_view: vk::ImageView,
    pub network_sampled_view: vk::ImageView,
    pub network_storage_view: vk::ImageView,
    pub dither_view: vk::ImageView,
    pub final_storage_view: vk::ImageView,
    /// Picture, private network, dither, private final image creation IDs.
    pub view_generations: [u64; 4],
}

fn info(buffer: vk::Buffer, offset: u64, range: u64) -> vk::DescriptorBufferInfo {
    vk::DescriptorBufferInfo {
        buffer,
        offset,
        range,
    }
}

pub(crate) fn shared(g: &Gpu) -> Result<&'static Kernels<'static>, String> {
    static SHARED: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();
    let pointer = SHARED
        .get_or_init(|| {
            // Gpu is the process-global Vulkan owner and outlives every submission.
            let owner: &'static Gpu = unsafe { &*(g as *const Gpu) };
            Kernels::new(owner).map(|k| Box::into_raw(Box::new(k)) as usize)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    Ok(unsafe { &*(*pointer as *const Kernels<'static>) })
}

/// Records the complete production graph. Every allocation belongs to `sub`, is
/// zeroed before use, and therefore retains borders and the sixteen spare rows.
pub fn record_graph(g: &Gpu, sub: &mut Sub, graph: GraphBuffers) -> Result<(), String> {
    prepare_graph(g, sub, graph)?.record(g, sub);
    Ok(())
}

pub(crate) fn prepare_graph(g: &Gpu, sub: &mut Sub, graph: GraphBuffers) -> Result<PreparedGraph, String> {
    let kernels = shared(g)?;
    let scratch = super::prepare::prepare_all(scratch_sizes(), |n| {
        let (buffer, offset) = sub
            .gpu_scratch(g, n)
            .ok_or_else(|| "NDVE tensor allocation".to_string())?;
        Ok::<_, String>(info(buffer, offset, n))
    })?;
    let jobs = graph_jobs(graph, &scratch[..8], scratch[8], scratch[9]);
    let dispatches = super::prepare::prepare_all(jobs, |(job, buffers, images)| {
        match images {
            Some((views, generations, pipe)) => kernels.prepare_images(sub, job, buffers, views, generations, pipe),
            None => kernels.prepare(sub, job, buffers),
        }
    })?;
    Ok(PreparedGraph { kernels, zero: scratch[..8].to_vec(), dispatches })
}

fn scratch_sizes() -> [u64; 10] {
    std::array::from_fn(|i| match i {
        8 => 256,
        9 => 1920 * 1080 * 2,
        _ => { let [w, h, c] = super::TENSORS[i];
            u64::from(w + 2) * u64::from(h + 2 + 16) * u64::from(c) * 2 },
    })
}

type PlannedJob = (Job, [vk::DescriptorBufferInfo; 5], Option<([vk::ImageView; 3], [u64; 3], usize)>);

fn graph_jobs(graph: GraphBuffers, tensors: &[vk::DescriptorBufferInfo],
    dummy: vk::DescriptorBufferInfo, luma: vk::DescriptorBufferInfo) -> Vec<PlannedJob> {
    let mut jobs = Vec::with_capacity(15);
    jobs.push((
        Job::new(Kernel::Pack, 960, 540).compare_rounding(),
        [graph.picture, tensors[0], dummy, dummy, dummy],
        None,
    ));
    for (i, layer) in super::LAYERS[1..12].iter().enumerate() {
        let [w, h, _] = if matches!(layer.kernel, Kernel::Up64 | Kernel::Up32 | Kernel::Out) {
            super::TENSORS[layer.input]
        } else if layer.output < super::TENSORS.len() {
            super::TENSORS[layer.output]
        } else {
            super::TENSORS[layer.input]
        };
        let output = if layer.output < tensors.len() {
            tensors[layer.output]
        } else {
            graph.network_output
        };
        let skip = layer.skip.map_or(graph.picture, |n| tensors[n]);
        jobs.push((
            Job::new(layer.kernel, w, h).compare_rounding(),
            [
                tensors[layer.input],
                output,
                graph.weights[i],
                graph.bias[i],
                skip,
            ],
            None,
        ));
    }
    jobs.push((
        Job::new(Kernel::Out, 960, 540).compare_rounding(),
        [
            tensors[0],
            graph.network_output,
            graph.weights[11],
            graph.bias[11],
            graph.picture,
        ],
        Some(([
            graph.picture_view,
            graph.picture_view,
            graph.network_storage_view,
        ], [graph.view_generations[0], graph.view_generations[0], graph.view_generations[1]], 2)),
    ));
    jobs.push((
        Job::new(Kernel::Luma, 1920, 1080).compare_rounding(),
        [graph.network_output, luma, dummy, dummy, dummy],
        Some(([
            graph.network_sampled_view,
            graph.network_sampled_view,
            graph.network_storage_view,
        ], [graph.view_generations[1]; 3], 2)),
    ));
    jobs.push((
        Job::new(Kernel::Resample, 1920, 1080).compare_rounding(),
        [
            luma,
            graph.final_output,
            graph.resample_weights,
            graph.dither,
            graph.network_output,
        ],
        Some(([
            graph.network_sampled_view,
            graph.dither_view,
            graph.final_storage_view,
        ], [graph.view_generations[1], graph.view_generations[2], graph.view_generations[3]], 3)),
    ));
    jobs
}

pub fn record_compare(
    g: &Gpu,
    sub: &mut Sub,
    width: u32,
    height: u32,
    a: vk::DescriptorBufferInfo,
    b: vk::DescriptorBufferInfo,
    count: vk::DescriptorBufferInfo,
) -> Result<(), String> {
    let dummy = if a.buffer != count.buffer { a } else { b };
    shared(g)?.record(
        sub,
        Job::new(Kernel::Compare, width, height),
        [a, count, dummy, dummy, b],
    )
}

pub fn record_image_compare(
    g: &Gpu,
    sub: &mut Sub,
    width: u32,
    height: u32,
    rgba8: bool,
    a: vk::DescriptorBufferInfo,
    b: vk::DescriptorBufferInfo,
    count: vk::DescriptorBufferInfo,
) -> Result<(), String> {
    let dummy = if a.buffer != count.buffer { a } else { b };
    shared(g)?.record(
        sub,
        Job::new(Kernel::Compare, width, height).image_compare(rgba8),
        [a, count, dummy, dummy, b],
    )
}

pub(crate) fn prepare_image_compare(
    g: &Gpu, sub: &mut Sub, width: u32, height: u32, rgba8: bool,
    a: vk::DescriptorBufferInfo, b: vk::DescriptorBufferInfo, count: vk::DescriptorBufferInfo,
) -> Result<PreparedDispatch, String> {
    let dummy = if a.buffer != count.buffer { a } else { b };
    shared(g)?.prepare(sub, Job::new(Kernel::Compare, width, height).image_compare(rgba8),
        [a, count, dummy, dummy, b])
}

impl<'g> Kernels<'g> {
    pub fn new(g: &'g Gpu) -> Result<Self, String> {
        Self::from_sources(g, [LAYER, RESAMPLE, OUT, RESAMPLE_IMAGE])
    }

    pub(super) fn from_sources(g: &'g Gpu, sources: [&[u8]; 4]) -> Result<Self, String> {
        let mut this = Self {
            g,
            set: vk::DescriptorSetLayout::null(),
            layout: vk::PipelineLayout::null(),
            pipes: [vk::Pipeline::null(); 4],
            sampler: vk::Sampler::null(),
            limits: g.d.props.limits,
            hashes: sources.map(crate::vk::queue::memo_hash),
        };
        let mut binds: Vec<_> = (0..5)
            .map(|i| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(i)
                    .descriptor_count(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE)
            })
            .collect();
        for (binding, ty) in [
            (5, vk::DescriptorType::COMBINED_IMAGE_SAMPLER),
            (6, vk::DescriptorType::COMBINED_IMAGE_SAMPLER),
            (7, vk::DescriptorType::STORAGE_IMAGE),
        ] {
            binds.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_count(1)
                    .descriptor_type(ty)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
            );
        }
        this.set = unsafe {
            g.dev().create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        this.sampler = unsafe {
            g.dev().create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::NEAREST)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let sets = [this.set];
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .size(32)];
        this.layout = unsafe {
            g.dev().create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&sets)
                    .push_constant_ranges(&ranges),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        for (i, bytes) in sources.into_iter().enumerate() {
            let words: Vec<u32> = bytes
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let m = unsafe {
                g.dev()
                    .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
            }
            .map_err(|e| e.to_string())?;
            let ci = vk::ComputePipelineCreateInfo::default()
                .layout(this.layout)
                .stage(
                    vk::PipelineShaderStageCreateInfo::default()
                        .stage(vk::ShaderStageFlags::COMPUTE)
                        .module(m)
                        .name(c"main"),
                );
            let result = unsafe {
                g.dev()
                    .create_compute_pipelines(vk::PipelineCache::null(), &[ci], None)
            };
            unsafe { g.dev().destroy_shader_module(m, None) };
            match result {
                Ok(v) => this.pipes[i] = v[0],
                Err((v, e)) => {
                    for p in v {
                        unsafe { g.dev().destroy_pipeline(p, None) };
                    }
                    return Err(format!("NDVE pipeline: {e}"));
                }
            }
        }
        Ok(this)
    }

    pub fn record(
        &self,
        sub: &mut Sub,
        job: Job,
        buffers: [vk::DescriptorBufferInfo; 5],
    ) -> Result<(), String> {
        let dispatch = self.prepare(sub, job, buffers)?;
        self.issue(sub, dispatch);
        Ok(())
    }

    fn validate(&self, job: Job, buffers: [vk::DescriptorBufferInfo; 5]) -> Result<[u32; 8], String> {
        let extents = job.extents()?;
        let args = job.args();
        let limits = &self.limits;
        super::prepare::validate_buffers(buffers.map(|b| super::prepare::BufferSpan {
            buffer: vk::Handle::as_raw(b.buffer), offset: b.offset, range: b.range,
        }), extents, limits.min_storage_buffer_offset_alignment,
            u64::from(limits.max_storage_buffer_range), args[5].div_ceil(64),
            limits.max_compute_work_group_count[0]).map_err(str::to_string)?;
        if limits.max_compute_work_group_invocations < 64 || limits.max_compute_work_group_size[0] < 64 {
            return Err("NDVE local size exceeds device limit".into());
        }
        if limits.max_push_constants_size < 32 || (job.kernel == Kernel::Resample
            && limits.max_compute_shared_memory_size < 4096) {
            return Err("NDVE push/shared memory exceeds device limit".into());
        }
        Ok(args)
    }

    pub(crate) fn prepare(
        &self,
        sub: &mut Sub,
        job: Job,
        buffers: [vk::DescriptorBufferInfo; 5],
    ) -> Result<PreparedDispatch, String> {
        let args = self.validate(job, buffers)?;
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(5),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(2),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
        ];
        let pool = unsafe {
            self.g.dev().create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let layouts = [self.set];
        let result = unsafe {
            self.g.dev().allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        };
        let set = match result {
            Ok(sets) => sets[0],
            Err(e) => {
                unsafe { self.g.dev().destroy_descriptor_pool(pool, None) };
                return Err(e.to_string());
            }
        };
        // One-use native pool: destroy only after this submission completes, not
        // in the guest pool reset/reuse list. Allocation failure above owns cleanup.
        sub.dpools.retain_transient(pool);
        let writes: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, b)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(i as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(b))
            })
            .collect();
        unsafe { self.g.dev().update_descriptor_sets(&writes, &[]) };
        let pipe = (job.kernel == Kernel::Resample) as usize;
        Ok(PreparedDispatch { set, pipe: self.pipes[pipe], args,
            evidence: self.evidence(sub, args, buffers, [vk::ImageView::null(); 3], pipe) })
    }

    pub(crate) fn issue(&self, sub: &mut Sub, dispatch: PreparedDispatch) {
        let PreparedDispatch { set, pipe, args, evidence } = dispatch;
        super::trace::record(evidence);
        let bytes: [u8; 32] = std::array::from_fn(|i| args[i / 4].to_le_bytes()[i % 4]);
        unsafe {
            let dev = self.g.dev();
            dev.cmd_bind_pipeline(
                sub.main,
                vk::PipelineBindPoint::COMPUTE,
                pipe,
            );
            dev.cmd_bind_descriptor_sets(
                sub.main,
                vk::PipelineBindPoint::COMPUTE,
                self.layout,
                0,
                &[set],
                &[],
            );
            dev.cmd_push_constants(
                sub.main,
                self.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                &bytes,
            );
            dev.cmd_dispatch(sub.main, args[5].div_ceil(64), 1, 1);
            let barriers = [vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)];
            dev.cmd_pipeline_barrier(
                sub.main,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &barriers,
                &[],
                &[],
            );
        }
    }

    fn prepare_images(
        &self,
        sub: &mut Sub,
        job: Job,
        buffers: [vk::DescriptorBufferInfo; 5],
        views: [vk::ImageView; 3],
        generations: [u64; 3],
        pipe: usize,
    ) -> Result<PreparedDispatch, String> {
        if views.contains(&vk::ImageView::null()) {
            return Err("NDVE missing image view".into());
        }
        let args = self.validate(job, buffers)?;
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(5),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(2),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
        ];
        let pool = unsafe {
            self.g.dev().create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&sizes),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let set = match unsafe {
            self.g.dev().allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&[self.set]),
            )
        } {
            Ok(sets) => sets[0],
            Err(e) => {
                unsafe { self.g.dev().destroy_descriptor_pool(pool, None) };
                return Err(e.to_string());
            }
        };
        sub.dpools.retain_transient(pool);
        let buffer_writes: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(binding, info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(binding as u32)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(info))
            })
            .collect();
        let sampled = [
            vk::DescriptorImageInfo::default()
                .sampler(self.sampler)
                .image_view(views[0])
                .image_layout(vk::ImageLayout::GENERAL),
            vk::DescriptorImageInfo::default()
                .sampler(self.sampler)
                .image_view(views[1])
                .image_layout(vk::ImageLayout::GENERAL),
        ];
        let storage = [vk::DescriptorImageInfo::default()
            .image_view(views[2])
            .image_layout(vk::ImageLayout::GENERAL)];
        let mut writes = buffer_writes;
        for binding in 5..=6 {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(binding)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&sampled[(binding - 5) as usize..(binding - 4) as usize]),
            );
        }
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(7)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&storage),
        );
        unsafe { self.g.dev().update_descriptor_sets(&writes, &[]) };
        let mut evidence = self.evidence(sub, args, buffers, views, pipe);
        evidence.image_generations = generations;
        Ok(PreparedDispatch { set, pipe: self.pipes[pipe], args, evidence })
    }

    fn evidence(&self, sub: &Sub, args: [u32; 8], buffers: [vk::DescriptorBufferInfo; 5],
        views: [vk::ImageView; 3], pipe: usize) -> super::trace::Record {
        super::trace::Record {
            serial: sub.serial, shader: self.hashes[pipe], groups: [args[5].div_ceil(64), 1, 1],
            local: [64, 1, 1], buffers: buffers.map(|b| [vk::Handle::as_raw(b.buffer), b.offset, b.range]),
            views: views.map(vk::Handle::as_raw),
            backing_generation: crate::vk::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire),
            ..Default::default()
        }
    }
}

impl Drop for Kernels<'_> {
    fn drop(&mut self) {
        // The owner must retain this object until its recorded submissions finish.
        unsafe {
            for p in self.pipes {
                self.g.dev().destroy_pipeline(p, None);
            }
            self.g.dev().destroy_sampler(self.sampler, None);
            self.g.dev().destroy_pipeline_layout(self.layout, None);
            self.g.dev().destroy_descriptor_set_layout(self.set, None);
        }
    }
}

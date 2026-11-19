//! Frame generation's LSFG mode: Lossless Scaling's LSFG 3.1 passes, read by the front end from the player's own
//! `Lossless.dll` (`front/src/lsfg.rs`; never shipped), run as Vulkan compute on the presented frames.
//!
//! The pass graph (what each pass reads and writes, image sizes and formats, samplers, the constant buffer) follows
//! lsfg-vk v1.0 (MIT; LICENSES/lsfg-vk.txt), the performance set. Per game frame: a mip chain of the frame's luma at
//! the flow scale (7 levels), the "alpha" passes per level (kept for the last three frames), the "beta" pass on the
//! finest. Per generated frame: the "gamma" passes from the coarsest level to the finest, the "delta" passes on the
//! three finest, then the generation pass that writes the frame at time t.
//!
//! The modules bind a constant buffer at 0, samplers from 16, sampled images from 32 and storage images from 48,
//! all in set 0; they are bound here with push descriptors. A pass's missing input (the first level's "previous"
//! result) reads a black 1x1 image.

use super::queue::Sub;
use super::Gpu;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use std::sync::{Arc, Mutex};

/// The shaders the front end found (SPIR-V words): the mip pass, the generation pass, then the 23 of the
/// performance set in lsfg-vk's order.
pub struct Shaders {
    pub mip: Vec<u32>,
    pub generate: Vec<u32>,
    pub passes: Vec<Vec<u32>>,
}

static SHADERS: Mutex<Option<Arc<Shaders>>> = Mutex::new(None);

/// The front end hands the identified set over (`None`: none usable).
pub fn set_shaders(s: Option<Arc<Shaders>>) {
    *SHADERS.lock().unwrap() = s;
}

pub fn shaders() -> Option<Arc<Shaders>> {
    SHADERS.lock().unwrap().clone()
}

pub fn shaders_ready() -> bool {
    SHADERS.lock().unwrap().is_some()
}

// the passes, in the set's order
const G0: usize = 0;
const D5: usize = 1;
const G1: usize = 2;
const G2: usize = 3;
const G3: usize = 4;
const G4: usize = 5;
const D1: usize = 6;
const D2: usize = 7;
const D3: usize = 8;
const D4: usize = 9;
const A0: usize = 10;
const A1: usize = 11;
const A2: usize = 12;
const A3: usize = 13;
const D6: usize = 14;
const D7: usize = 15;
const D8: usize = 16;
const D9: usize = 17;
const B0: usize = 18;
const B1: usize = 19;
const B2: usize = 20;
const B3: usize = 21;
const B4: usize = 22;
const MIP: usize = 23;
const GEN: usize = 24;

struct Prog {
    pipe: vk::Pipeline,
    layout: vk::PipelineLayout,
    ubo: bool,
    ns: u32,
    ni: u32,
    no: u32,
}

#[derive(Clone, Copy)]
struct Img {
    image: vk::Image,
    view: vk::ImageView,
    w: u32,
    h: u32,
}

const RGBA8: vk::Format = vk::Format::R8G8B8A8_UNORM;
const R8: vk::Format = vk::Format::R8_UNORM;
const RGBA16F: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

struct Alpha {
    t1: Img,
    t2: Img,
    t3: [Img; 2],
    out: [[Img; 2]; 3],
}
struct Beta {
    t1: [Img; 2],
    t2: [Img; 2],
    out: [Img; 6],
}
struct Gamma {
    t1: [Img; 3],
    t2: [Img; 2],
    out: Img,
}
struct Delta {
    t1: [Img; 3],
    t2: [Img; 2],
    out1: Img,
    out2: Img,
}

struct Ctx {
    w: u32,
    h: u32,
    flow: f32,
    inp: [Img; 2],
    gen: [Img; 3],
    mip: [Img; 7],
    a: Vec<Alpha>,
    b: Beta,
    g: Vec<Gamma>,
    d: Vec<Delta>,
    fc: u64,
    owned: Vec<(vk::Image, vk::ImageView, Allocation)>,
    fresh: Vec<vk::Image>,
}

pub struct Lsfg {
    progs: Vec<Prog>,
    samplers: [vk::Sampler; 3],
    black: Img,
    black_alloc: Option<Allocation>,
    black_fresh: bool,
    ubos: Vec<([f32; 3], super::Buf)>,
    ctx: Option<Ctx>,
}

fn make(g: &Gpu, w: u32, h: u32, f: vk::Format, owned: &mut Vec<(vk::Image, vk::ImageView, Allocation)>, fresh: &mut Vec<vk::Image>) -> Result<Img, String> {
    let (w, h) = (w.max(1), h.max(1));
    let dev = g.dev();
    let ci = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(f)
        .extent(vk::Extent3D { width: w, height: h, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST);
    let image = unsafe { dev.create_image(&ci, None) }.map_err(|e| format!("lsfg image: {e}"))?;
    let req = unsafe { dev.get_image_memory_requirements(image) };
    let a = g
        .alloc
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc { name: "lsfg", requirements: req, location: MemoryLocation::GpuOnly, linear: false, allocation_scheme: AllocationScheme::GpuAllocatorManaged })
        .map_err(|e| format!("lsfg image: {e}"))?;
    unsafe { dev.bind_image_memory(image, a.memory(), a.offset()) }.map_err(|e| e.to_string())?;
    let vci = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(f)
        .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 });
    let view = unsafe { dev.create_image_view(&vci, None) }.map_err(|e| e.to_string())?;
    owned.push((image, view, a));
    fresh.push(image);
    Ok(Img { image, view, w, h })
}

/// Bindings a module declares at 0 (its constant buffer).
fn has_ubo(w: &[u32]) -> bool {
    let mut i = 5;
    while i < w.len() {
        let (op, n) = (w[i] & 0xffff, (w[i] >> 16) as usize);
        if n == 0 {
            break;
        }
        if op == 71 && n >= 4 && w[i + 2] == 33 && w[i + 3] == 0 {
            return true;
        }
        if op == 54 {
            break;
        }
        i += n;
    }
    false
}

fn counts(w: &[u32]) -> (u32, u32, u32) {
    let (mut s, mut im, mut o) = (0u32, 0u32, 0u32);
    let mut i = 5;
    while i < w.len() {
        let (op, n) = (w[i] & 0xffff, (w[i] >> 16) as usize);
        if n == 0 || op == 54 {
            break;
        }
        if op == 71 && n >= 4 && w[i + 2] == 33 {
            let b = w[i + 3];
            match b {
                16..=31 => s = s.max(b - 15),
                32..=47 => im = im.max(b - 31),
                48.. => o = o.max(b - 47),
                _ => {}
            }
        }
        i += n;
    }
    (s, im, o)
}

impl Lsfg {
    pub fn new(g: &Gpu, sh: &Shaders) -> Result<Lsfg, String> {
        if !g.d.caps.push_desc {
            return Err("the device has no VK_KHR_push_descriptor".into());
        }
        if sh.passes.len() != 23 {
            return Err(format!("{} passes, not 23", sh.passes.len()));
        }
        let dev = g.dev();
        let mut progs = Vec::new();
        let all: Vec<&Vec<u32>> = sh.passes.iter().chain([&sh.mip, &sh.generate]).collect();
        for w in all {
            let (ns, ni, no) = counts(w);
            let ubo = has_ubo(w);
            let mut binds = vec![];
            if ubo {
                binds.push(vk::DescriptorSetLayoutBinding::default().binding(0).descriptor_type(vk::DescriptorType::UNIFORM_BUFFER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE));
            }
            for k in 0..ns {
                binds.push(vk::DescriptorSetLayoutBinding::default().binding(16 + k).descriptor_type(vk::DescriptorType::SAMPLER).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE));
            }
            for k in 0..ni {
                binds.push(vk::DescriptorSetLayoutBinding::default().binding(32 + k).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE));
            }
            for k in 0..no {
                binds.push(vk::DescriptorSetLayoutBinding::default().binding(48 + k).descriptor_type(vk::DescriptorType::STORAGE_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE));
            }
            let dl = unsafe {
                dev.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().flags(vk::DescriptorSetLayoutCreateFlags::PUSH_DESCRIPTOR_KHR).bindings(&binds), None)
            }
            .map_err(|e| e.to_string())?;
            let dls = [dl];
            let layout = unsafe { dev.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&dls), None) }.map_err(|e| e.to_string())?;
            let m = unsafe { dev.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(w), None) }.map_err(|e| e.to_string())?;
            let st = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::COMPUTE).module(m).name(c"main");
            let pipe = unsafe { dev.create_compute_pipelines(vk::PipelineCache::null(), &[vk::ComputePipelineCreateInfo::default().stage(st).layout(layout)], None) }
                .map_err(|(_, e)| format!("LSFG pipeline: {e}"))?[0];
            unsafe { dev.destroy_shader_module(m, None) };
            progs.push(Prog { pipe, layout, ubo, ns, ni, no });
        }
        let mk_s = |mode: vk::SamplerAddressMode, border: vk::BorderColor| {
            let ci = vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .address_mode_u(mode)
                .address_mode_v(mode)
                .address_mode_w(mode)
                .border_color(border)
                .max_lod(0.0);
            unsafe { dev.create_sampler(&ci, None) }.map_err(|e| e.to_string())
        };
        let samplers = [
            mk_s(vk::SamplerAddressMode::CLAMP_TO_BORDER, vk::BorderColor::FLOAT_TRANSPARENT_BLACK)?,
            mk_s(vk::SamplerAddressMode::CLAMP_TO_BORDER, vk::BorderColor::FLOAT_OPAQUE_WHITE)?,
            mk_s(vk::SamplerAddressMode::CLAMP_TO_EDGE, vk::BorderColor::FLOAT_TRANSPARENT_BLACK)?,
        ];
        let mut owned = vec![];
        let mut fresh = vec![];
        let black = make(g, 1, 1, RGBA8, &mut owned, &mut fresh)?;
        let (_, _, ba) = owned.pop().unwrap();
        Ok(Lsfg { progs, samplers, black, black_alloc: Some(ba), black_fresh: true, ubos: vec![], ctx: None })
    }

    fn ctx(&mut self, g: &Gpu, w: u32, h: u32, flow: f32) -> Result<(), String> {
        if self.ctx.as_ref().is_some_and(|c| c.w == w && c.h == h && c.flow == flow) {
            return Ok(());
        }
        if let Some(c) = self.ctx.take() {
            super::queue::wait_fence(g.q.last_fence(), u64::MAX);
            for (i, v, a) in c.owned {
                unsafe {
                    g.dev().destroy_image_view(v, None);
                    g.dev().destroy_image(i, None);
                }
                let _ = g.alloc.lock().unwrap().free(a);
            }
        }
        let mut owned = vec![];
        let mut fresh = vec![];
        let mut mk = |w: u32, h: u32, f: vk::Format| make(g, w, h, f, &mut owned, &mut fresh);
        let inp = [mk(w, h, RGBA8)?, mk(w, h, RGBA8)?];
        let gen = [mk(w, h, RGBA8)?, mk(w, h, RGBA8)?, mk(w, h, RGBA8)?];
        let (fw, fh) = ((w as f32 * flow) as u32, (h as f32 * flow) as u32);
        let mut mip = vec![];
        for i in 0..7 {
            mip.push(mk(fw >> i, fh >> i, R8)?);
        }
        let mip: [Img; 7] = mip.try_into().ok().unwrap();
        let mut a = vec![];
        for m in mip.iter() {
            let (hw, hh) = ((m.w + 1) >> 1, (m.h + 1) >> 1);
            let (qw, qh) = ((hw + 1) >> 1, (hh + 1) >> 1);
            a.push(Alpha {
                t1: mk(hw, hh, RGBA8)?,
                t2: mk(hw, hh, RGBA8)?,
                t3: [mk(qw, qh, RGBA8)?, mk(qw, qh, RGBA8)?],
                out: [[mk(qw, qh, RGBA8)?, mk(qw, qh, RGBA8)?], [mk(qw, qh, RGBA8)?, mk(qw, qh, RGBA8)?], [mk(qw, qh, RGBA8)?, mk(qw, qh, RGBA8)?]],
            });
        }
        let (bw, bh) = (a[0].out[0][0].w, a[0].out[0][0].h);
        let b = Beta {
            t1: [mk(bw, bh, RGBA8)?, mk(bw, bh, RGBA8)?],
            t2: [mk(bw, bh, RGBA8)?, mk(bw, bh, RGBA8)?],
            out: [mk(bw, bh, R8)?, mk(bw >> 1, bh >> 1, R8)?, mk(bw >> 2, bh >> 2, R8)?, mk(bw >> 3, bh >> 3, R8)?, mk(bw >> 4, bh >> 4, R8)?, mk(bw >> 5, bh >> 5, R8)?],
        };
        let mut gm = vec![];
        for i in 0..7 {
            let (w, h) = (a[6 - i].out[0][0].w, a[6 - i].out[0][0].h);
            gm.push(Gamma { t1: [mk(w, h, RGBA8)?, mk(w, h, RGBA8)?, mk(w, h, RGBA8)?], t2: [mk(w, h, RGBA8)?, mk(w, h, RGBA8)?], out: mk(w, h, RGBA16F)? });
        }
        let mut d = vec![];
        for j in 0..3 {
            let i = 4 + j;
            let (w, h) = (a[6 - i].out[0][0].w, a[6 - i].out[0][0].h);
            d.push(Delta { t1: [mk(w, h, RGBA8)?, mk(w, h, RGBA8)?, mk(w, h, RGBA8)?], t2: [mk(w, h, RGBA8)?, mk(w, h, RGBA8)?], out1: mk(w, h, RGBA16F)?, out2: mk(w, h, RGBA16F)? });
        }
        crate::vklog!("frame generation: LSFG context {w}x{h}, flow {fw}x{fh}, {} images", owned.len());
        self.ctx = Some(Ctx { w, h, flow, inp, gen, mip, a, b, g: gm, d, fc: 0, owned, fresh });
        Ok(())
    }

    fn ubo(&mut self, g: &Gpu, ts: f32, first: bool, first_s: bool, inv_flow: f32) -> vk::Buffer {
        let key = [ts, first as u32 as f32, first_s as u32 as f32];
        if let Some((_, b)) = self.ubos.iter().find(|(k, _)| *k == key) {
            return b.buf;
        }
        let b = g.buffer(48, vk::BufferUsageFlags::UNIFORM_BUFFER, MemoryLocation::CpuToGpu, "lsfg constants").expect("lsfg constants");
        // offset (2), first iteration, first iteration (s), colours, hdr, 1 / flow scale, timestamp, ui threshold
        let mut v = [0u32; 12];
        v[2] = first as u32;
        v[3] = first_s as u32;
        v[6] = inv_flow.to_bits();
        v[7] = ts.to_bits();
        v[8] = 0.5f32.to_bits();
        unsafe { core::ptr::copy_nonoverlapping(v.as_ptr() as *const u8, b.ptr, 48) };
        let buf = b.buf;
        self.ubos.push((key, b));
        buf
    }

    #[allow(clippy::too_many_arguments)]
    fn d(&self, g: &Gpu, cb: vk::CommandBuffer, p: usize, ubo: Option<vk::Buffer>, inputs: &[Option<Img>], smp: &[usize], outs: &[Img], gx: u32, gy: u32) {
        let pr = &self.progs[p];
        let dev = g.dev();
        let bi = ubo.map(|b| [vk::DescriptorBufferInfo { buffer: b, offset: 0, range: 48 }]);
        let si: Vec<[vk::DescriptorImageInfo; 1]> =
            (0..pr.ns as usize).map(|k| [vk::DescriptorImageInfo { sampler: self.samplers[*smp.get(k).unwrap_or(&smp[0])], ..Default::default() }]).collect();
        let ii: Vec<[vk::DescriptorImageInfo; 1]> = (0..pr.ni as usize)
            .map(|k| [vk::DescriptorImageInfo { image_view: inputs.get(k).copied().flatten().unwrap_or(self.black).view, image_layout: vk::ImageLayout::GENERAL, ..Default::default() }])
            .collect();
        let oi: Vec<[vk::DescriptorImageInfo; 1]> =
            (0..pr.no as usize).map(|k| [vk::DescriptorImageInfo { image_view: outs.get(k).unwrap_or(&self.black).view, image_layout: vk::ImageLayout::GENERAL, ..Default::default() }]).collect();
        let mut w: Vec<vk::WriteDescriptorSet> = vec![];
        if pr.ubo {
            if let Some(bi) = &bi {
                w.push(vk::WriteDescriptorSet::default().dst_binding(0).descriptor_type(vk::DescriptorType::UNIFORM_BUFFER).buffer_info(bi));
            }
        }
        for (k, x) in si.iter().enumerate() {
            w.push(vk::WriteDescriptorSet::default().dst_binding(16 + k as u32).descriptor_type(vk::DescriptorType::SAMPLER).image_info(x));
        }
        for (k, x) in ii.iter().enumerate() {
            w.push(vk::WriteDescriptorSet::default().dst_binding(32 + k as u32).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(x));
        }
        for (k, x) in oi.iter().enumerate() {
            w.push(vk::WriteDescriptorSet::default().dst_binding(48 + k as u32).descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(x));
        }
        unsafe {
            dev.cmd_bind_pipeline(cb, vk::PipelineBindPoint::COMPUTE, pr.pipe);
            g.d.push_desc.cmd_push_descriptor_set(cb, vk::PipelineBindPoint::COMPUTE, pr.layout, 0, &w);
            dev.cmd_dispatch(cb, gx.max(1), gy.max(1), 1);
        }
        super::queue::full_barrier(g, cb);
    }

    /// One game frame: the copy into the input ring (`blit` from `src`'s region, turned upright), the per-frame
    /// passes, and `ngen` generated frames. Returns the generated images (oldest first) and the frame's own copy.
    #[allow(clippy::too_many_arguments)]
    pub fn process(&mut self, g: &Gpu, sub: &mut Sub, src: vk::Image, r: [i32; 4], ngen: usize, flow: f32) -> Result<(Vec<vk::Image>, vk::Image, u32, u32), String> {
        let (w, h) = ((r[2] - r[0]).unsigned_abs(), (r[3] - r[1]).unsigned_abs());
        self.ctx(g, w, h, flow)?;
        let cb = sub.main;
        let dev = g.dev();
        // new images to GENERAL
        let mut fresh = std::mem::take(&mut self.ctx.as_mut().unwrap().fresh);
        if self.black_fresh {
            fresh.push(self.black.image);
        }
        if !fresh.is_empty() {
            let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
            let b: Vec<_> = fresh
                .iter()
                .map(|&im| {
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(vk::PipelineStageFlags2::NONE)
                        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::GENERAL)
                        .image(im)
                        .subresource_range(range)
                })
                .collect();
            unsafe { dev.cmd_pipeline_barrier2(cb, &vk::DependencyInfo::default().image_memory_barriers(&b)) };
            if self.black_fresh {
                unsafe { dev.cmd_clear_color_image(cb, self.black.image, vk::ImageLayout::GENERAL, &vk::ClearColorValue { float32: [0.0; 4] }, &[range]) };
                self.black_fresh = false;
            }
            super::queue::full_barrier(g, cb);
        }
        let inv_flow = 1.0 / flow;
        let c = self.ctx.as_ref().unwrap();
        let fc = c.fc;
        let input = c.inp[(fc % 2) as usize];
        let blit = vk::ImageBlit {
            src_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
            src_offsets: [vk::Offset3D { x: r[0], y: r[1], z: 0 }, vk::Offset3D { x: r[2], y: r[3], z: 1 }],
            dst_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
            dst_offsets: [vk::Offset3D::default(), vk::Offset3D { x: w as i32, y: h as i32, z: 1 }],
        };
        unsafe { dev.cmd_blit_image(cb, src, vk::ImageLayout::GENERAL, input.image, vk::ImageLayout::GENERAL, &[blit], vk::Filter::NEAREST) };
        super::queue::full_barrier(g, cb);
        let q = |n: u32, d: u32| n.div_ceil(d);
        let u0 = self.ubo(g, 0.0, false, false, inv_flow);
        let u_half = self.ubo(g, 0.5, false, false, inv_flow);
        let c = self.ctx.as_ref().unwrap();
        // 1) per game frame: mips, alpha (7 levels), beta
        self.d(g, cb, MIP, Some(u0), &[Some(input)], &[0], &c.mip, q(c.mip[0].w, 64), q(c.mip[0].h, 64));
        let slot = (fc % 3) as usize;
        for i in (0..7).rev() {
            let a = &c.a[i];
            let (hw, hh, qw, qh) = (a.t1.w, a.t1.h, a.t3[0].w, a.t3[0].h);
            self.d(g, cb, A0, None, &[Some(c.mip[i])], &[0], &[a.t1], q(hw, 8), q(hh, 8));
            self.d(g, cb, A1, None, &[Some(a.t1)], &[0], &[a.t2], q(hw, 8), q(hh, 8));
            self.d(g, cb, A2, None, &[Some(a.t2)], &[0], &a.t3, q(qw, 8), q(qh, 8));
            self.d(g, cb, A3, None, &[Some(a.t3[0]), Some(a.t3[1])], &[0], &a.out[slot], q(qw, 8), q(qh, 8));
        }
        {
            let b = &c.b;
            let a = &c.a[0];
            let (bw, bh) = (b.t1[0].w, b.t1[0].h);
            let (n1, n2) = ((slot + 1) % 3, (slot + 2) % 3);
            let ins = [Some(a.out[n1][0]), Some(a.out[n1][1]), Some(a.out[n2][0]), Some(a.out[n2][1]), Some(a.out[slot][0]), Some(a.out[slot][1])];
            self.d(g, cb, B0, None, &ins, &[1], &b.t1, q(bw, 8), q(bh, 8));
            self.d(g, cb, B1, None, &[Some(b.t1[0]), Some(b.t1[1])], &[0], &b.t2, q(bw, 8), q(bh, 8));
            self.d(g, cb, B2, None, &[Some(b.t2[0]), Some(b.t2[1])], &[0], &b.t1, q(bw, 8), q(bh, 8));
            self.d(g, cb, B3, None, &[Some(b.t1[0]), Some(b.t1[1])], &[0], &b.t2, q(bw, 8), q(bh, 8));
            self.d(g, cb, B4, Some(u_half), &[Some(b.t2[0]), Some(b.t2[1])], &[0], &b.out, q(bw, 32), q(bh, 32));
        }
        let (cw, chh) = (c.w, c.h);
        let prev_in = c.inp[((fc + 1) % 2) as usize];
        self.ctx.as_mut().unwrap().fc += 1;
        let mut gens = vec![];
        if fc < 2 || ngen == 0 {
            return Ok((gens, input.image, w, h));
        }
        // 2) per generated frame: gamma (7 levels) + delta (the 3 finest) + generate
        let (cur, prv) = (slot, ((fc + 2) % 3) as usize);
        for pass in 0..ngen.min(3) {
            let ts = (pass + 1) as f32 / (ngen + 1) as f32;
            let ubos: Vec<vk::Buffer> = (0..7).map(|i| self.ubo(g, ts, i == 0, false, inv_flow)).collect();
            let ubos_d: Vec<vk::Buffer> = (0..3).map(|j| self.ubo(g, ts, false, j == 0, inv_flow)).collect();
            let ug = self.ubo(g, ts, false, false, inv_flow);
            let c = self.ctx.as_ref().unwrap();
            for i in 0..7 {
                let gm = &c.g[i];
                let a = &c.a[6 - i];
                let opt = if i > 0 { Some(c.g[i - 1].out) } else { None };
                let (gw, gh) = (gm.t1[0].w, gm.t1[0].h);
                let inb = c.b.out[(6 - i).min(5)];
                let ub = ubos[i];
                self.d(g, cb, G0, Some(ub), &[Some(a.out[prv][0]), Some(a.out[prv][1]), Some(a.out[cur][0]), Some(a.out[cur][1]), opt], &[1, 2], &gm.t1, q(gw, 8), q(gh, 8));
                self.d(g, cb, G1, None, &[Some(gm.t1[0]), Some(gm.t1[1]), Some(gm.t1[2])], &[0], &gm.t2, q(gw, 8), q(gh, 8));
                self.d(g, cb, G2, None, &[Some(gm.t2[0]), Some(gm.t2[1])], &[0], &gm.t1[..2], q(gw, 8), q(gh, 8));
                self.d(g, cb, G3, None, &[Some(gm.t1[0]), Some(gm.t1[1])], &[0], &gm.t2, q(gw, 8), q(gh, 8));
                self.d(g, cb, G4, Some(ub), &[Some(gm.t2[0]), Some(gm.t2[1]), opt, Some(inb)], &[0, 2], &[gm.out], q(gw, 8), q(gh, 8));
                if i < 4 {
                    continue;
                }
                let j = i - 4;
                let dl = &c.d[j];
                let o1 = if j > 0 { Some(c.g[i - 1].out) } else { None };
                let o2 = if j > 0 { Some(c.d[j - 1].out1) } else { None };
                let o3 = if j > 0 { Some(c.d[j - 1].out2) } else { None };
                let inb2 = c.b.out[6 - i];
                let ud = ubos_d[j];
                self.d(g, cb, G0, Some(ud), &[Some(a.out[prv][0]), Some(a.out[prv][1]), Some(a.out[cur][0]), Some(a.out[cur][1]), o1], &[1, 2], &dl.t1, q(gw, 8), q(gh, 8));
                self.d(g, cb, D1, None, &[Some(dl.t1[0]), Some(dl.t1[1]), Some(dl.t1[2])], &[0], &dl.t2, q(gw, 8), q(gh, 8));
                self.d(g, cb, D2, None, &[Some(dl.t2[0]), Some(dl.t2[1])], &[0], &dl.t1[..2], q(gw, 8), q(gh, 8));
                self.d(g, cb, D3, None, &[Some(dl.t1[0]), Some(dl.t1[1])], &[0], &dl.t2, q(gw, 8), q(gh, 8));
                self.d(g, cb, D4, Some(ud), &[Some(dl.t2[0]), Some(dl.t2[1]), o1, Some(inb2)], &[0, 2], &[dl.out1], q(gw, 8), q(gh, 8));
                self.d(g, cb, D5, Some(ud), &[Some(a.out[prv][0]), Some(a.out[prv][1]), Some(a.out[cur][0]), Some(a.out[cur][1]), o1, o2], &[1, 2], &[dl.t2[0]], q(gw, 8), q(gh, 8));
                self.d(g, cb, D6, None, &[Some(dl.t2[0])], &[0], &[dl.t1[0]], q(gw, 8), q(gh, 8));
                self.d(g, cb, D7, None, &[Some(dl.t1[0])], &[0], &[dl.t2[0]], q(gw, 8), q(gh, 8));
                self.d(g, cb, D8, None, &[Some(dl.t2[0])], &[0], &[dl.t1[0]], q(gw, 8), q(gh, 8));
                self.d(g, cb, D9, Some(ud), &[Some(dl.t1[0]), o3], &[0, 2], &[dl.out2], q(gw, 8), q(gh, 8));
            }
            let out = c.gen[pass];
            self.d(g, cb, GEN, Some(ug), &[Some(prev_in), Some(input), Some(c.g[6].out), Some(c.d[2].out1), Some(c.d[2].out2)], &[0, 2], &[out], q(cw, 16), q(chh, 16));
            gens.push(out.image);
        }
        Ok((gens, input.image, w, h))
    }
}

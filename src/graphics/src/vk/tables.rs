//! The pool descriptor set: NVN's texture and sampler pools as bindless arrays the shaders index by host ID (the
//! active pool's base plus the ID in the handle). Registration writes the descriptor at once (update-after-bind):
//! like the console's descriptor memory, an ID the GPU still reads must not be rewritten by the game.

use super::device::Device;
use super::layout::{self, Layout};
use crate::nvn::obj::{samp_par, view_t};
use ash::vk;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct Tables {
    pub pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    samplers: HashMap<[u32; 10], vk::Sampler>,
    tex_end: i32,
    samp_end: i32,
    /// host texture ID -> (serial, view) as registered
    tex: HashMap<i32, (u64, Option<view_t>)>,
    samp: HashMap<i32, vk::Sampler>,
    /// buffer textures by host texture ID (no image: a texel buffer view over their pool's memory)
    buf_tex: HashMap<i32, crate::nvn::obj::tex_t>,
    /// buffer textures whose pool-set texel buffer views are still to be made (at the next submission)
    buf_pending: Vec<i32>,
    /// the pool set's texel buffer views by ID (destroyed when the ID is registered again)
    buf_views: HashMap<i32, (vk::BufferView, Option<vk::BufferView>)>,
}

fn samp_key(p: &samp_par) -> [u32; 10] {
    [
        u32::from_le_bytes([p.minf, p.magf, p.ws, p.wt]),
        u32::from_le_bytes([p.wr, p.cmp_mode, p.cmp_func, p.reduction]),
        p.lod_min.to_bits(),
        p.lod_max.to_bits(),
        p.lod_bias.to_bits(),
        p.aniso.to_bits(),
        p.border[0].to_bits(),
        p.border[1].to_bits(),
        p.border[2].to_bits(),
        p.border[3].to_bits(),
    ]
}

fn wrap(w: u8) -> vk::SamplerAddressMode {
    match w {
        0 | 7 => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        1 => vk::SamplerAddressMode::REPEAT,
        2 | 3 | 4 => vk::SamplerAddressMode::MIRROR_CLAMP_TO_EDGE,
        5 => vk::SamplerAddressMode::CLAMP_TO_BORDER,
        6 => vk::SamplerAddressMode::MIRRORED_REPEAT,
        _ => vk::SamplerAddressMode::REPEAT,
    }
}

/// NVN compare functions (NEVER = 1 .. ALWAYS = 8) as Vulkan's (NEVER = 0 .. ALWAYS = 7).
pub fn compare_op(f: u32) -> vk::CompareOp {
    vk::CompareOp::from_raw((f.clamp(1, 8) - 1) as i32)
}

impl Tables {
    pub fn new(d: &Device, l: &Layout) -> Result<Tables, String> {
        let sizes = [
            vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: l.tex_cap },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLER, descriptor_count: l.samp_cap },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_IMAGE, descriptor_count: l.tex_cap },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_TEXEL_BUFFER, descriptor_count: l.tex_cap },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_TEXEL_BUFFER, descriptor_count: l.tex_cap },
        ];
        let ci = vk::DescriptorPoolCreateInfo::default().flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND).max_sets(1).pool_sizes(&sizes);
        let pool = unsafe { d.dev.create_descriptor_pool(&ci, None) }.map_err(|e| format!("pool set pool: {e}"))?;
        let sl = [l.pool_set];
        let ai = vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&sl);
        let set = unsafe { d.dev.allocate_descriptor_sets(&ai) }.map_err(|e| format!("pool set: {e}"))?[0];
        Ok(Tables { pool, set, inner: Mutex::new(Inner::default()) })
    }

    /// A buffer texture registered at host ID `id`.
    pub fn note_buf_tex(&self, id: i32, t: crate::nvn::obj::tex_t) {
        let mut i = self.inner.lock().unwrap();
        i.tex.remove(&id);
        i.buf_tex.insert(id, t);
        i.buf_pending.push(id);
    }
    pub fn buf_tex_at(&self, id: i32) -> Option<crate::nvn::obj::tex_t> {
        self.inner.lock().unwrap().buf_tex.get(&id).copied()
    }
    pub fn take_buf_pending(&self) -> Vec<i32> {
        std::mem::take(&mut self.inner.lock().unwrap().buf_pending)
    }
    /// Writes the pool set's texel buffer views of buffer texture `id` (uniform, and storage when the format allows).
    pub fn write_texel_views(&self, d: &Device, id: i32, uv: vk::BufferView, sv: Option<vk::BufferView>) {
        let u = [uv];
        let s = [sv.unwrap_or(uv)];
        let mut w = vec![vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(layout::B_TBUF).dst_array_element(id as u32).descriptor_type(vk::DescriptorType::UNIFORM_TEXEL_BUFFER).texel_buffer_view(&u)];
        if sv.is_some() {
            w.push(vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(layout::B_IBUF).dst_array_element(id as u32).descriptor_type(vk::DescriptorType::STORAGE_TEXEL_BUFFER).texel_buffer_view(&s));
        }
        let mut g = self.inner.lock().unwrap();
        unsafe { d.dev.update_descriptor_sets(&w, &[]) };
        if let Some((ou, os)) = g.buf_views.insert(id, (uv, sv)) {
            drop(g);
            if let Some(gpu) = super::Gpu::try_get() {
                gpu.q.defer_after_use(Box::new(move |gpu: &super::Gpu| unsafe {
                    gpu.dev().destroy_buffer_view(ou, None);
                    if let Some(os) = os {
                        gpu.dev().destroy_buffer_view(os, None);
                    }
                }));
            }
        }
    }

    pub fn reserve_tex(&self, base: i32, n: i32) {
        let g = super::Gpu::try_get();
        let cap = g.map(|g| g.layout.tex_cap as i32).unwrap_or(i32::MAX);
        let mut i = self.inner.lock().unwrap();
        i.tex_end = i.tex_end.max(base + n);
        if base + n > cap {
            crate::log::red(&format!("texture pool IDs up to {} pass the pool set's {cap} (NXGFX_TEX_CAP)", base + n));
        }
    }
    pub fn reserve_samp(&self, base: i32, n: i32) {
        let g = super::Gpu::try_get();
        let cap = g.map(|g| g.layout.samp_cap as i32).unwrap_or(i32::MAX);
        let mut i = self.inner.lock().unwrap();
        i.samp_end = i.samp_end.max(base + n);
        if base + n > cap {
            crate::log::red(&format!("sampler pool IDs up to {} pass the pool set's {cap} (NXGFX_SAMP_CAP)", base + n));
        }
    }

    /// The (shared) Vulkan sampler of NVN sampler parameters.
    pub fn sampler(&self, d: &Device, p: &samp_par) -> vk::Sampler {
        let k = samp_key(p);
        let mut i = self.inner.lock().unwrap();
        if let Some(s) = i.samplers.get(&k) {
            return *s;
        }
        let (min, mip, nomip) = match p.minf {
            0 => (vk::Filter::NEAREST, vk::SamplerMipmapMode::NEAREST, true),
            1 => (vk::Filter::LINEAR, vk::SamplerMipmapMode::NEAREST, true),
            2 => (vk::Filter::NEAREST, vk::SamplerMipmapMode::NEAREST, false),
            3 => (vk::Filter::LINEAR, vk::SamplerMipmapMode::NEAREST, false),
            4 => (vk::Filter::NEAREST, vk::SamplerMipmapMode::LINEAR, false),
            _ => (vk::Filter::LINEAR, vk::SamplerMipmapMode::LINEAR, false),
        };
        let mag = if p.magf == 0 { vk::Filter::NEAREST } else { vk::Filter::LINEAR };
        let min_lod = p.lod_min.max(0.0);
        let max_lod = if nomip { min_lod + 0.25 } else { p.lod_max.max(min_lod) };
        let aniso = p.aniso > 1.0;
        let border_rgba = p.border;
        let mut cbc = vk::SamplerCustomBorderColorCreateInfoEXT::default()
            .custom_border_color(vk::ClearColorValue { float32: border_rgba })
            .format(vk::Format::UNDEFINED);
        let standard = |b: [f32; 4]| {
            if b[3] == 0.0 {
                vk::BorderColor::FLOAT_TRANSPARENT_BLACK
            } else if b[0] == 0.0 && b[1] == 0.0 && b[2] == 0.0 {
                vk::BorderColor::FLOAT_OPAQUE_BLACK
            } else {
                vk::BorderColor::FLOAT_OPAQUE_WHITE
            }
        };
        let is_std = border_rgba == [0.0; 4] || border_rgba == [0.0, 0.0, 0.0, 1.0] || border_rgba == [1.0; 4];
        let mut ci = vk::SamplerCreateInfo::default()
            .mag_filter(mag)
            .min_filter(min)
            .mipmap_mode(mip)
            .address_mode_u(wrap(p.ws))
            .address_mode_v(wrap(p.wt))
            .address_mode_w(wrap(p.wr))
            .mip_lod_bias(p.lod_bias.clamp(-15.0, 15.0))
            .anisotropy_enable(aniso)
            .max_anisotropy(p.aniso.clamp(1.0, 16.0))
            .compare_enable(p.cmp_mode != 0)
            .compare_op(compare_op(p.cmp_func as u32))
            .min_lod(min_lod)
            .max_lod(max_lod)
            .border_color(if is_std || !d.caps.custom_border { standard(border_rgba) } else { vk::BorderColor::FLOAT_CUSTOM_EXT });
        if !is_std && d.caps.custom_border {
            ci = ci.push_next(&mut cbc);
        }
        let mut red = vk::SamplerReductionModeCreateInfo::default().reduction_mode(match p.reduction {
            1 => vk::SamplerReductionMode::MIN,
            2 => vk::SamplerReductionMode::MAX,
            _ => vk::SamplerReductionMode::WEIGHTED_AVERAGE,
        });
        if p.reduction != 0 {
            ci = ci.push_next(&mut red);
        }
        let s = unsafe { d.dev.create_sampler(&ci, None) }.unwrap_or_else(|e| panic!("vkCreateSampler: {e}"));
        if i.samplers.len() == 3500 {
            crate::log::red("3,500 distinct samplers: close to the driver's limit of sampler objects");
        }
        i.samplers.insert(k, s);
        s
    }

    pub fn note_tex(&self, id: i32, serial: u64, view: Option<view_t>) {
        self.inner.lock().unwrap().tex.insert(id, (serial, view));
    }
    pub fn tex_at(&self, id: i32) -> Option<(u64, Option<view_t>)> {
        self.inner.lock().unwrap().tex.get(&id).copied()
    }
    pub fn samp_at(&self, id: i32) -> Option<vk::Sampler> {
        self.inner.lock().unwrap().samp.get(&id).copied()
    }

    pub fn write_sampler(&self, d: &Device, id: i32, s: vk::Sampler) {
        self.inner.lock().unwrap().samp.insert(id, s);
        let ii = [vk::DescriptorImageInfo::default().sampler(s)];
        let w = [vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(layout::B_SAMP).dst_array_element(id as u32).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&ii)];
        let _g = self.inner.lock().unwrap();
        unsafe { d.dev.update_descriptor_sets(&w, &[]) };
    }

    pub fn write_image(&self, d: &Device, id: i32, view: vk::ImageView, storage: Option<vk::ImageView>) {
        let ii = [vk::DescriptorImageInfo::default().image_view(view).image_layout(vk::ImageLayout::GENERAL)];
        let si = [vk::DescriptorImageInfo::default().image_view(storage.unwrap_or(view)).image_layout(vk::ImageLayout::GENERAL)];
        let mut w = vec![vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(layout::B_TEX).dst_array_element(id as u32).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&ii)];
        if storage.is_some() {
            w.push(vk::WriteDescriptorSet::default().dst_set(self.set).dst_binding(layout::B_IMG).dst_array_element(id as u32).descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(&si));
        }
        let _g = self.inner.lock().unwrap();
        unsafe { d.dev.update_descriptor_sets(&w, &[]) };
    }
}

impl super::Gpu {
    /// nvnSamplerPoolRegisterSampler at host ID `id`.
    pub fn reg_samp(&self, id: i32, p: &samp_par) {
        if id < 0 || id as u32 >= self.layout.samp_cap {
            crate::log::red_once(&format!("sampler ID {id} past the pool set"));
            return;
        }
        let s = self.tables.sampler(&self.d, p);
        self.tables.write_sampler(&self.d, id, s);
    }
}

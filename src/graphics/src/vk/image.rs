//! Textures as Vulkan images, found by the texture's initialization serial (a game may copy NVNtexture objects by
//! value). Images stay in GENERAL layout for their whole life.
//!
//! Content: a texture's storage is its content until the GPU writes it. A new image is filled from its storage
//! (deswizzled from block-linear on the CPU; ASTC decoded on the GPU and kept as RGBA8, or BC7 at 256x256 and up);
//! CPU writes to the storage of a texture the GPU has not written make it upload again (`mem` reports the pages).
//! Render targets, copy destinations, cleared and storage images are GPU-owned: their storage no longer counts.

use super::queue::Sub;
use super::Gpu;
use crate::nvn::obj::{tex_t, view_t, VW_FORMAT, VW_LAYERS, VW_LEVELS, VW_SWIZZLE, VW_TARGET};
use crate::nvn::types::{target, tex_flags, NVNcopyRegion};
use crate::tex::format::{self, Kind, TexFormat};
use crate::tex::{deswizzle, layout};
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct ViewKey {
    pub view: view_t,
    pub has: bool,
    /// an attachment view: identity swizzle, every aspect
    pub attach: bool,
    /// a storage-image view: identity swizzle (storage descriptors take no swizzle)
    pub storage: bool,
}

pub struct Image {
    pub tex: tex_t,
    pub image: vk::Image,
    pub alloc: Option<Allocation>,
    pub format: vk::Format,
    pub tf: TexFormat,
    pub ty: vk::ImageType,
    pub extent: vk::Extent3D,
    pub levels: u32,
    pub layers: u32,
    pub samples: vk::SampleCountFlags,
    pub usage: vk::ImageUsageFlags,
    pub views: HashMap<ViewKey, vk::ImageView>,
    pub gpu_owned: bool,
    pub needs_init: bool,
    pub needs_upload: bool,
    pub bc7: bool,
    /// a texture in a sparse virtual pool: the physical ranges its last upload read and the mapping generation then
    pub sparse: Option<(Vec<(u64, u64)>, u64)>,
    /// a hash of each 4 KB page of the storage as last uploaded: a CPU write to a page the texture shares with other
    /// data (a uniform ring, a neighbour) that leaves its bytes as they were uploads nothing
    pub page_hash: Vec<u64>,
}
unsafe impl Send for Image {}

const HPAGE: u64 = 4096;

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut ch = b.chunks_exact(8);
    for c in &mut ch {
        h = (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(0x100000001b3).rotate_left(23);
    }
    for &x in ch.remainder() {
        h = (h ^ x as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// The hash of page `i` of [lo, hi) (clipped to the range).
fn page_hash_at(lo: u64, hi: u64, i: usize) -> u64 {
    let s = (lo & !(HPAGE - 1)) + i as u64 * HPAGE;
    let (a, e) = (s.max(lo), (s + HPAGE).min(hi));
    if e <= a {
        return 0;
    }
    // SAFETY: the texture's storage in guest memory
    hash_bytes(unsafe { core::slice::from_raw_parts(a as *const u8, (e - a) as usize) })
}

/// Hashes of every page of a texture's storage.
fn page_hashes(lo: u64, hi: u64) -> Vec<u64> {
    let n = ((hi + HPAGE - 1) / HPAGE - lo / HPAGE) as usize;
    (0..n).map(|i| page_hash_at(lo, hi, i)).collect()
}

/// Whether a write to [a, a + n) changed a page of the texture's storage [lo, hi) since its hashes were taken (the
/// changed pages' hashes are updated).
fn pages_changed(hashes: &mut [u64], lo: u64, hi: u64, a: u64, n: u64) -> bool {
    if hashes.is_empty() {
        return true;
    }
    let base = lo & !(HPAGE - 1);
    let (s, e) = (a.max(lo), (a + n).min(hi));
    if e <= s {
        return false;
    }
    let mut changed = false;
    for i in ((s - base) / HPAGE) as usize..=((e - 1 - base) / HPAGE) as usize {
        if i >= hashes.len() {
            break;
        }
        let h = page_hash_at(lo, hi, i);
        if h != hashes[i] {
            hashes[i] = h;
            changed = true;
        }
    }
    changed
}

#[derive(Default)]
pub struct Images {
    pub by_serial: HashMap<u64, Image>,
    /// storage start -> serials (CPU-written textures)
    by_addr: BTreeMap<u64, Vec<u64>>,
    max_size: u64,
    /// texel writes waiting for the next submission: (serial, level, layer, offset, extent, linear rows)
    writes: Vec<(u64, u32, u32, vk::Offset3D, vk::Extent3D, Vec<u8>)>,
    /// the virtual pool mapping generation the sparse textures were last checked at
    sparse_seen: u64,
    /// released images by what they were (storage, description, layout): a game that initializes its transient
    /// render targets again every frame over the same memory gets the same image back, with its content, instead of
    /// a new one uploaded from storage (Mario Kart 8 Deluxe: ~2 a frame, 15 ms of uploads). (image, present frame)
    parked: HashMap<u128, Vec<(Image, u64)>>,
    parked_n: usize,
}

/// What a texture object is, without its serial: the key a released image is found again by.
fn tex_identity(t: &tex_t) -> u128 {
    // SAFETY: tex_t is plain data of 0x80 bytes; bytes 4..0x78 are everything but the magic and the serial
    let b = unsafe { core::slice::from_raw_parts(t as *const tex_t as *const u8, 0x78) };
    super::queue::memo_hash(&b[4..])
}

const PARK_FRAMES: u64 = 4;
const PARK_MAX: usize = 512;

impl Images {
    pub fn drop_pool(&mut self, _base: u64, _size: u64) {}
}

fn view_type(t: i32) -> vk::ImageViewType {
    match t {
        target::T1D => vk::ImageViewType::TYPE_1D,
        target::T1D_ARRAY => vk::ImageViewType::TYPE_1D_ARRAY,
        target::T3D => vk::ImageViewType::TYPE_3D,
        target::T2D_ARRAY | target::T2D_MS_ARRAY => vk::ImageViewType::TYPE_2D_ARRAY,
        target::CUBE => vk::ImageViewType::CUBE,
        target::CUBE_ARRAY => vk::ImageViewType::CUBE_ARRAY,
        _ => vk::ImageViewType::TYPE_2D,
    }
}

fn swz(v: u16) -> vk::ComponentSwizzle {
    match v {
        0 => vk::ComponentSwizzle::ZERO,
        1 => vk::ComponentSwizzle::ONE,
        2 => vk::ComponentSwizzle::R,
        3 => vk::ComponentSwizzle::G,
        4 => vk::ComponentSwizzle::B,
        5 => vk::ComponentSwizzle::A,
        _ => vk::ComponentSwizzle::IDENTITY,
    }
}

/// The texture's channel mapping composed with a view's, and alpha 1 for formats without alpha.
fn components(t: &tex_t, v: Option<&view_t>, tf: &TexFormat) -> vk::ComponentMapping {
    let s = t.swizzle();
    let mut c = [s & 7, s >> 3 & 7, s >> 6 & 7, s >> 9 & 7];
    if let Some(v) = v {
        if v.bits & VW_SWIZZLE != 0 {
            let base = c;
            for i in 0..4 {
                let w = v.swz[i] as u16;
                c[i] = if (2..=5).contains(&w) { base[(w - 2) as usize] } else { w };
            }
        }
    }
    if tf.alpha_one {
        for x in c.iter_mut() {
            if *x == 5 {
                *x = 1;
            }
        }
    }
    vk::ComponentMapping { r: swz(c[0]), g: swz(c[1]), b: swz(c[2]), a: swz(c[3]) }
}

fn shape(t: &tex_t) -> (vk::ImageType, vk::Extent3D, u32) {
    let d = &t.d;
    let w = d.width.max(1);
    let h = d.height.max(1);
    let z = d.depth.max(1);
    match d.target {
        target::T1D => (vk::ImageType::TYPE_1D, vk::Extent3D { width: w, height: 1, depth: 1 }, 1),
        target::T1D_ARRAY => (vk::ImageType::TYPE_1D, vk::Extent3D { width: w, height: 1, depth: 1 }, h),
        target::T3D => (vk::ImageType::TYPE_3D, vk::Extent3D { width: w, height: h, depth: z }, 1),
        target::T2D_ARRAY | target::T2D_MS_ARRAY | target::CUBE_ARRAY => (vk::ImageType::TYPE_2D, vk::Extent3D { width: w, height: h, depth: 1 }, z),
        target::CUBE => (vk::ImageType::TYPE_2D, vk::Extent3D { width: w, height: h, depth: 1 }, 6),
        _ => (vk::ImageType::TYPE_2D, vk::Extent3D { width: w, height: h, depth: 1 }, 1),
    }
}

fn samples_of(n: i32) -> vk::SampleCountFlags {
    match n {
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        _ => vk::SampleCountFlags::TYPE_1,
    }
}

pub fn aspect(tf: &TexFormat) -> vk::ImageAspectFlags {
    format::aspect_of(tf.kind)
}

impl Gpu {
    fn create_image(&self, t: &tex_t) -> Option<Image> {
        let Some(mut tf) = format::tex_format(t.d.format) else {
            crate::log::red_once(&format!("texture format {:#x} has no Vulkan format", t.d.format));
            return None;
        };
        if t.d.target == target::BUFFER {
            crate::log::red_once("buffer textures are not implemented yet");
            return None;
        }
        let (ty, extent, layers) = shape(t);
        let levels = t.d.levels.max(1);
        let mut bc7 = false;
        if let Some((_, _, srgb)) = tf.astc {
            let big = extent.width >= 256 && extent.height >= 256 && std::env::var("NXGFX_ASTC").as_deref() != Ok("rgba8");
            if big && !self.d.format_features(vk::Format::BC7_UNORM_BLOCK).is_empty() {
                tf.vk = if srgb { vk::Format::BC7_SRGB_BLOCK } else { vk::Format::BC7_UNORM_BLOCK };
                bc7 = true;
            }
        }
        let feat = self.d.format_features(tf.vk);
        if feat.is_empty() {
            crate::log::red_once(&format!("format {:?} (NVN {:#x}) is not supported by the device", tf.vk, t.d.format));
            return None;
        }
        let mut usage = vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST;
        if feat.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE) {
            usage |= vk::ImageUsageFlags::SAMPLED;
        }
        if feat.contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT) {
            usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
        }
        if feat.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT) {
            usage |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
        }
        let samples = samples_of(t.d.samples);
        if feat.contains(vk::FormatFeatureFlags::STORAGE_IMAGE) && (t.d.flags as u32 & tex_flags::IMAGE != 0) && samples == vk::SampleCountFlags::TYPE_1 {
            usage |= vk::ImageUsageFlags::STORAGE;
        }
        let mut flags = vk::ImageCreateFlags::empty();
        let compressed = tf.astc.is_none() && layout::fmt_bpb(t.d.format).1 > 1 || bc7;
        if !compressed && tf.kind != Kind::Depth && tf.kind != Kind::DepthStencil && tf.kind != Kind::Stencil {
            flags |= vk::ImageCreateFlags::MUTABLE_FORMAT;
        }
        // any square 2D image of whole cubes: games view 2D arrays as cubes and cubes as cube arrays
        if ty == vk::ImageType::TYPE_2D && extent.width == extent.height && layers >= 6 && layers % 6 == 0 && samples == vk::SampleCountFlags::TYPE_1 {
            flags |= vk::ImageCreateFlags::CUBE_COMPATIBLE;
        }
        let ci = vk::ImageCreateInfo::default()
            .flags(flags)
            .image_type(ty)
            .format(tf.vk)
            .extent(extent)
            .mip_levels(levels)
            .array_layers(layers)
            .samples(samples)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = match unsafe { self.dev().create_image(&ci, None) } {
            Ok(i) => i,
            Err(e) => {
                crate::log::red(&format!("vkCreateImage {:?} {}x{}x{} levels {levels} layers {layers}: {e}", tf.vk, extent.width, extent.height, extent.depth));
                return None;
            }
        };
        let req = unsafe { self.dev().get_image_memory_requirements(image) };
        let a = self
            .alloc
            .lock()
            .unwrap()
            .allocate(&AllocationCreateDesc { name: "texture", requirements: req, location: MemoryLocation::GpuOnly, linear: false, allocation_scheme: AllocationScheme::GpuAllocatorManaged })
            .ok()?;
        unsafe { self.dev().bind_image_memory(image, a.memory(), a.offset()) }.ok()?;
        Some(Image {
            tex: *t,
            image,
            alloc: Some(a),
            format: tf.vk,
            tf,
            ty,
            extent,
            levels,
            layers,
            samples,
            usage,
            views: HashMap::new(),
            gpu_owned: false,
            needs_init: true,
            needs_upload: !t.mem.is_null(),
            bc7,
            sparse: None,
            page_hash: Vec::new(),
        })
    }

    /// The image of a texture, created at its first use.
    pub fn ensure_image(&self, imgs: &mut Images, t: &tex_t) -> bool {
        if imgs.by_serial.contains_key(&t.serial) {
            return true;
        }
        // the same texture initialized again over the same memory: its released image, with its content
        let key = tex_identity(t);
        let reused = imgs.parked.get_mut(&key).and_then(|v| v.pop()).map(|(mut im, _)| {
            imgs.parked_n -= 1;
            im.tex = *t;
            if !im.gpu_owned && im.sparse.is_none() && !im.page_hash.is_empty() && !t.mem.is_null() {
                let lo = t.mem as u64;
                let mut ph = std::mem::take(&mut im.page_hash);
                im.needs_upload |= pages_changed(&mut ph, lo, lo + t.l.size, lo, t.l.size);
                im.page_hash = ph;
            } else if !im.gpu_owned {
                im.needs_upload = !t.mem.is_null();
            }
            super::perf::inc(super::perf::C::ImagesReused);
            im
        });
        let Some(im) = reused.or_else(|| self.create_image(t)) else { return false };
        if !t.mem.is_null() {
            imgs.by_addr.entry(t.mem as u64).or_default().push(t.serial);
            imgs.max_size = imgs.max_size.max(t.l.size);
        }
        imgs.by_serial.insert(t.serial, im);
        true
    }

    /// A view of an image (created once per view key).
    pub fn image_view(&self, im: &mut Image, v: Option<&view_t>, attach: bool) -> Option<vk::ImageView> {
        self.image_view_kind(im, v, attach, false)
    }

    /// The format a view of `im` through `v` has (the texture's, or the view's own when Vulkan allows it).
    pub fn view_format(&self, im: &Image, v: Option<&view_t>) -> vk::Format {
        let f = self.raw_view_format(im, v);
        // a compressed image keeps its own format in views (an uncompressed view of its blocks is a render target's
        // block alias, exec::attach_view)
        let compressed = im.bc7 || layout::fmt_bpb(im.tex.d.format).1 > 1;
        if compressed && f != im.format {
            im.format
        } else {
            f
        }
    }

    /// The view's own format as Vulkan allows it, before the compressed-image rule.
    pub fn raw_view_format(&self, im: &Image, v: Option<&view_t>) -> vk::Format {
        let mut fmt = im.format;
        if let Some(v) = v {
            if v.bits & VW_FORMAT != 0 && im.tf.astc.is_none() {
                if let Some(f) = format::tex_format(v.format as i32) {
                    if im.usage.contains(vk::ImageUsageFlags::STORAGE) || f.vk == fmt || !matches!(f.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                        fmt = f.vk;
                    }
                }
            }
        }
        fmt
    }

    /// A storage-image view (identity swizzle).
    pub fn storage_view(&self, im: &mut Image, v: Option<&view_t>) -> Option<vk::ImageView> {
        self.image_view_kind(im, v, false, true)
    }

    fn image_view_kind(&self, im: &mut Image, v: Option<&view_t>, attach: bool, storage: bool) -> Option<vk::ImageView> {
        let k = ViewKey { view: v.copied().unwrap_or_default(), has: v.is_some(), attach, storage };
        if let Some(x) = im.views.get(&k) {
            return Some(*x);
        }
        let t = im.tex;
        let mut ty = view_type(t.d.target);
        let mut fmt = im.format;
        let (mut base_level, mut nlevels, mut base_layer, mut nlayers) = (0, im.levels, 0, im.layers);
        let mut asp = aspect(&im.tf);
        if let Some(v) = v {
            if v.bits & VW_LEVELS != 0 {
                base_level = v.base_level as u32;
                nlevels = (v.nlevels as u32).max(1);
            }
            if v.bits & VW_LAYERS != 0 {
                base_layer = v.min_layer as u32;
                nlayers = (v.nlayers as u32).max(1);
            }
            if v.bits & VW_TARGET != 0 {
                ty = view_type(v.target as i32);
            }
            fmt = self.view_format(im, Some(v));
            if v.bits & 0x20 != 0 && (v.bits >> 6) == 1 && im.tf.kind == Kind::DepthStencil {
                asp = vk::ImageAspectFlags::STENCIL;
            }
        }
        if !attach && asp == (vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL) {
            asp = if t.bits & 2 != 0 { vk::ImageAspectFlags::STENCIL } else { vk::ImageAspectFlags::DEPTH };
        }
        if attach {
            if ty == vk::ImageViewType::CUBE || ty == vk::ImageViewType::CUBE_ARRAY {
                ty = vk::ImageViewType::TYPE_2D_ARRAY;
            }
            nlevels = 1;
            if im.ty == vk::ImageType::TYPE_3D {
                ty = vk::ImageViewType::TYPE_3D;
            }
        }
        if ty == vk::ImageViewType::TYPE_2D && nlayers > 1 {
            nlayers = 1;
        }
        // a cube view of a 2D array: six faces from its first layer (whole cubes for a cube array)
        if ty == vk::ImageViewType::CUBE {
            nlayers = 6;
        } else if ty == vk::ImageViewType::CUBE_ARRAY {
            nlayers = (nlayers.min(im.layers.saturating_sub(base_layer)) / 6 * 6).max(6);
        }
        if base_level + nlevels > im.levels {
            nlevels = im.levels.saturating_sub(base_level).max(1);
        }
        let comps = if attach || storage { vk::ComponentMapping::default() } else { components(&t, v, &im.tf) };
        let want = if attach {
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
        } else if storage {
            vk::ImageUsageFlags::STORAGE
        } else {
            vk::ImageUsageFlags::SAMPLED
        };
        let mut vusage = im.usage & want;
        // a view format the image's own format cannot have as an attachment or storage (a compressed image's
        // uncompressed view): the view keeps the usages its format allows
        let ff = self.d.format_features(fmt);
        if !ff.contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT) {
            vusage &= !vk::ImageUsageFlags::COLOR_ATTACHMENT;
        }
        if !ff.contains(vk::FormatFeatureFlags::STORAGE_IMAGE) {
            vusage &= !vk::ImageUsageFlags::STORAGE;
        }
        if vusage.is_empty() {
            crate::log::red_once(&format!("a view of a {:?} image as {fmt:?} has no usage the image allows ({})", im.format, if attach { "attachment" } else if storage { "storage" } else { "sampled" }));
            return None;
        }
        let usage_ci = vk::ImageViewUsageCreateInfo::default().usage(vusage);
        let mut usage_ci = usage_ci;
        let ci = vk::ImageViewCreateInfo::default()
            .image(im.image)
            .view_type(ty)
            .format(fmt)
            .components(comps)
            .subresource_range(vk::ImageSubresourceRange { aspect_mask: asp, base_mip_level: base_level, level_count: nlevels, base_array_layer: base_layer, layer_count: nlayers })
            .push_next(&mut usage_ci);
        let view = match unsafe { self.dev().create_image_view(&ci, None) } {
            Ok(v) => v,
            Err(e) => {
                crate::log::red(&format!("vkCreateImageView: {e}"));
                return None;
            }
        };
        im.views.insert(k, view);
        Some(view)
    }

    /// nvnTexturePoolRegisterTexture / RegisterImage at host ID `id`.
    pub fn reg_tex(&self, id: i32, t: &tex_t, v: Option<view_t>, as_image: bool) {
        if !t.valid() {
            crate::log::red_once("RegisterTexture of an uninitialized texture object");
            return;
        }
        if id < 0 || id as u32 >= self.layout.tex_cap {
            crate::log::red_once(&format!("texture ID {id} past the pool set (NXGFX_TEX_CAP)"));
            return;
        }
        if t.d.target == target::BUFFER {
            // a buffer texture: no image; its texel buffer views are made over its pool's memory
            self.tables.note_buf_tex(id, *t);
            return;
        }
        let mut imgs = self.images.lock().unwrap();
        if !self.ensure_image(&mut imgs, t) {
            return;
        }
        let im = imgs.by_serial.get_mut(&t.serial).unwrap();
        let storage = im.usage.contains(vk::ImageUsageFlags::STORAGE) && (as_image || t.d.flags as u32 & tex_flags::IMAGE != 0);
        if as_image {
            im.gpu_owned = true;
        }
        let Some(view) = self.image_view(im, v.as_ref(), false) else { return };
        let sview = if storage { self.storage_view(im, v.as_ref()) } else { None };
        drop(imgs);
        self.tables.note_tex(id, t.serial, v);
        self.tables.write_image(&self.d, id, view, sview);
    }

    /// nvnTextureFinalize: the image goes once the GPU no longer uses it.
    pub fn tex_release(&self, serial: u64) {
        let mut imgs = self.images.lock().unwrap();
        let Some(im) = imgs.by_serial.remove(&serial) else { return };
        if let Some(v) = imgs.by_addr.get_mut(&(im.tex.mem as u64)) {
            v.retain(|s| *s != serial);
        }
        // kept a few frames for a texture initialized again the same way (ensure_image)
        let key = tex_identity(&im.tex);
        let frame = super::present::frames();
        imgs.parked.entry(key).or_default().push((im, frame));
        imgs.parked_n += 1;
        let old = self.evict_parked(&mut imgs, frame);
        drop(imgs);
        for im in old {
            self.destroy_released(im);
        }
    }

    /// Parked images older than PARK_FRAMES frames, or the oldest past PARK_MAX: taken out to be destroyed.
    fn evict_parked(&self, imgs: &mut Images, frame: u64) -> Vec<Image> {
        let mut out = Vec::new();
        if imgs.parked_n == 0 {
            return out;
        }
        let over = imgs.parked_n > PARK_MAX;
        for v in imgs.parked.values_mut() {
            let mut i = 0;
            while i < v.len() {
                if frame.saturating_sub(v[i].1) > PARK_FRAMES || over && v.len() > 1 && i == 0 {
                    out.push(v.remove(i).0);
                } else {
                    i += 1;
                }
            }
        }
        imgs.parked.retain(|_, v| !v.is_empty());
        imgs.parked_n -= out.len();
        out
    }

    fn destroy_released(&self, im: Image) {
        {
            self.q.defer_after_use(
                Box::new(move |g: &Gpu| unsafe {
                    for v in im.views.values() {
                        g.dev().destroy_image_view(*v, None);
                    }
                    g.dev().destroy_image(im.image, None);
                    if let Some(a) = im.alloc {
                        let _ = g.alloc.lock().unwrap().free(a);
                    }
                }),
            );
        }
    }

    /// The GPU writes a texture: its storage no longer holds its content.
    pub fn tex_gpu_written(&self, imgs: &mut Images, serial: u64) {
        if let Some(im) = imgs.by_serial.get_mut(&serial) {
            im.gpu_owned = true;
            im.needs_upload = false;
        }
    }

    /// Start of a submission: textures whose storage the CPU wrote upload again; new images get their layout and
    /// content; texel writes land. All recorded into the submission's upload command buffer.
    /// A texel buffer view of buffer texture `t` (its format, `width` texels from its storage address), kept until
    /// `sub` completes; None when its memory or format cannot be viewed. `storage`: a storage texel buffer view.
    pub fn texel_view(&self, sub: &mut Sub, t: &tex_t, storage: bool) -> Option<vk::BufferView> {
        let Some(tf) = format::tex_format(t.d.format) else {
            crate::log::red_once(&format!("buffer texture format {:#x} has no texel buffer format", t.d.format));
            return None;
        };
        let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
        if bw != 1 || bh != 1 || bpb == 0 || tf.expand3 != 0 {
            crate::log::red_once(&format!("buffer texture format {:?} cannot be a texel buffer", tf.vk));
            return None;
        }
        let need = if storage { vk::FormatFeatureFlags::STORAGE_TEXEL_BUFFER } else { vk::FormatFeatureFlags::UNIFORM_TEXEL_BUFFER };
        let ff = unsafe { self.d.instance.get_physical_device_format_properties(self.d.pdev, tf.vk) }.buffer_features;
        if !ff.contains(need) {
            if !storage {
                crate::log::red_once(&format!("buffer texture format {:?} is not a texel buffer format on this device", tf.vk));
            }
            return None;
        }
        let max = self.d.props.limits.max_texel_buffer_elements as u64;
        let n = (t.d.width as u64).min(max) * bpb as u64;
        if n == 0 || t.mem.is_null() {
            return None;
        }
        let (buf, off, _) = super::mem::resolve(self, sub, t.mem as u64, n)?;
        if off % self.d.props.limits.min_texel_buffer_offset_alignment != 0 {
            crate::log::red_once(&format!("buffer texture at an offset Vulkan cannot view ({off:#x})"));
            return None;
        }
        let ci = vk::BufferViewCreateInfo::default().buffer(buf).format(tf.vk).offset(off).range(n);
        let v = unsafe { self.dev().create_buffer_view(&ci, None) }.ok()?;
        sub.buffer_views.push(v);
        Some(v)
    }

    fn texel_view_persistent(&self, sub: &mut Sub, t: &tex_t, storage: bool) -> Option<vk::BufferView> {
        let v = self.texel_view(sub, t, storage)?;
        // the pool set holds it past this submission: it is taken out of the submission's list and kept
        sub.buffer_views.pop();
        Some(v)
    }

    pub fn image_begin_submit(&self, sub: &mut Sub, dirty: &[(u64, u64)]) {
        // buffer textures registered since the last submission: their pool-set views
        for id in self.tables.take_buf_pending() {
            let Some(t) = self.tables.buf_tex_at(id) else { continue };
            let Some(uv) = self.texel_view_persistent(sub, &t, false) else { continue };
            let sv = self.texel_view_persistent(sub, &t, true);
            self.tables.write_texel_views(&self.d, id, uv, sv);
        }
        let mut imgs = self.images.lock().unwrap();
        if !dirty.is_empty() && !imgs.by_addr.is_empty() {
            let max = imgs.max_size;
            let mut hit = Vec::new();
            for &(a, n) in dirty {
                for (_, ss) in imgs.by_addr.range(a.saturating_sub(max)..a + n) {
                    hit.extend_from_slice(ss);
                }
            }
            for s in hit {
                if let Some(im) = imgs.by_serial.get_mut(&s) {
                    let lo = im.tex.mem as u64;
                    let hi = lo + im.tex.l.size;
                    let covered: u64 = dirty.iter().map(|&(a, n)| (a + n).min(hi).saturating_sub(a.max(lo))).sum();
                    // a GPU-written texture is taken back from its storage only when the CPU rewrote most of it (a
                    // decoded movie frame); a write to a page it merely shares keeps the GPU's content
                    // bytes the texture already holds (a write to a page it shares with other data): nothing to upload
                    if covered > 0 && !im.gpu_owned && !im.needs_upload && im.sparse.is_none() && !im.page_hash.is_empty() {
                        let mut ph = std::mem::take(&mut im.page_hash);
                        let changed = dirty.iter().fold(false, |c, &(a, n)| pages_changed(&mut ph, lo, hi, a, n) || c);
                        im.page_hash = ph;
                        if !changed {
                            super::perf::inc(super::perf::C::UploadsSame);
                            continue;
                        }
                    }
                    if covered > 0 && (!im.gpu_owned || covered * 2 >= hi - lo) {
                        // the CPU wrote the storage: its bytes are the texture's newest content, also when the GPU
                        // wrote the texture before (a movie frame the game decodes into a texture it cleared once:
                        // Mario Kart 8 Deluxe's mode-select preview stayed black)
                        if im.gpu_owned {
                            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                crate::vklog!("a texture the GPU wrote was written by the CPU: uploaded again from its storage (later ones are not logged)");
                            }
                            im.gpu_owned = false;
                        }
                        im.needs_upload = true;
                    }
                }
            }
        }
        // textures of sparse virtual pools: a mapping in their range changed, or the CPU wrote a physical page behind them
        let vgen = crate::nvn::vpool::generation();
        let look = vgen != imgs.sparse_seen;
        imgs.sparse_seen = vgen;
        for im in imgs.by_serial.values_mut() {
            let Some((runs, gen)) = im.sparse.as_ref() else { continue };
            if im.gpu_owned {
                continue;
            }
            let hit = dirty.iter().any(|&(a, n)| runs.iter().any(|&(b, m)| a < b + m && b < a + n));
            if hit || (look && crate::nvn::vpool::gen_in(im.tex.mem as u64, im.tex.l.size) > *gen) {
                im.needs_upload = true;
            }
        }
        let serials: Vec<u64> = imgs.by_serial.iter().filter(|(_, i)| i.needs_init || i.needs_upload).map(|(s, _)| *s).collect();
        for s in serials {
            self.prepare_image(&mut imgs, s, sub);
        }
        let writes = std::mem::take(&mut imgs.writes);
        for (s, level, layer, off, ext, data) in writes {
            if let Some(im) = imgs.by_serial.get(&s) {
                let (buf, bo) = sub.stage_bytes(self, &data);
                let r = vk::BufferImageCopy {
                    buffer_offset: bo,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers { aspect_mask: aspect(&im.tf), mip_level: level, base_array_layer: layer, layer_count: 1 },
                    image_offset: off,
                    image_extent: ext,
                };
                unsafe { self.dev().cmd_copy_buffer_to_image(sub.pre, buf, im.image, vk::ImageLayout::GENERAL, &[r]) };
            }
        }
    }

    /// Layout and contents of an image, in the submission's upload command buffer (also used mid-replay for a render
    /// target created there: the upload command buffer runs before the replay).
    pub fn prepare_image(&self, imgs: &mut Images, serial: u64, sub: &mut Sub) {
        let Some(im) = imgs.by_serial.get_mut(&serial) else { return };
        let range = vk::ImageSubresourceRange { aspect_mask: aspect(&im.tf), base_mip_level: 0, level_count: im.levels, base_array_layer: 0, layer_count: im.layers };
        if im.needs_init {
            let b = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(im.image)
                .subresource_range(range)];
            unsafe { self.dev().cmd_pipeline_barrier2(sub.pre, &vk::DependencyInfo::default().image_memory_barriers(&b)) };
            // defined content (zero) for what no upload covers
            let is_ds = matches!(im.tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil);
            let compressed = im.bc7 || layout::fmt_bpb(im.tex.d.format).1 > 1 && im.tf.astc.is_none();
            if !compressed {
                if is_ds {
                    let v = vk::ClearDepthStencilValue { depth: 0.0, stencil: 0 };
                    unsafe { self.dev().cmd_clear_depth_stencil_image(sub.pre, im.image, vk::ImageLayout::GENERAL, &v, &[range]) };
                } else {
                    let v = vk::ClearColorValue { uint32: [0; 4] };
                    unsafe { self.dev().cmd_clear_color_image(sub.pre, im.image, vk::ImageLayout::GENERAL, &v, &[range]) };
                }
                super::queue::full_barrier(self, sub.pre);
            }
            im.needs_init = false;
        }
        if im.needs_upload && !im.gpu_owned && !im.tex.mem.is_null() {
            let t0 = std::time::Instant::now();
            self.upload_from_storage(im, sub);
            if im.sparse.is_none() {
                let lo = im.tex.mem as u64;
                im.page_hash = page_hashes(lo, lo + im.tex.l.size);
            }
            let ns = t0.elapsed().as_nanos() as u64;
            super::perf::add(super::perf::C::TexUploadNs, ns);
            upload_stats::note(serial, &im.tex, ns);
            super::queue::full_barrier(self, sub.pre);
        }
        im.needs_upload = false;
    }

    fn upload_from_storage(&self, im: &mut Image, sub: &mut Sub) {
        let t = im.tex;
        let d = t.d;
        // a texture of a sparse virtual pool reads its storage through the pool's page table (unmapped pages as 0)
        let owned;
        let storage: &[u8] = if crate::nvn::vpool::is_sparse(t.mem as u64) {
            let mut v = vec![0u8; t.l.size as usize];
            let gen = crate::nvn::vpool::generation();
            let runs = crate::nvn::vpool::copy(t.mem as u64, &mut v).unwrap_or_default();
            im.sparse = Some((runs, gen));
            owned = v;
            &owned
        } else {
            super::lazy::touch(t.mem as u64, t.l.size);
            unsafe { core::slice::from_raw_parts(t.mem as *const u8, t.l.size as usize) }
        };
        let (bpb, bw, bh) = layout::fmt_bpb(d.format);
        let linear = d.flags as u32 & (tex_flags::LINEAR | tex_flags::LINEAR_RENDER_TARGET) != 0;
        let mut copies = Vec::new();
        for layer in 0..im.layers {
            for level in 0..im.levels {
                let lw = (im.extent.width >> level).max(1);
                let lh = (im.extent.height >> level).max(1);
                let ld = (im.extent.depth >> level).max(1);
                let (rows_data, row_bytes, rows, slices) = if linear {
                    if level > 0 {
                        continue;
                    }
                    let stride = if d.stride != 0 { d.stride as usize } else { (t.l.layer_size / lh.max(1) as u64) as usize };
                    let rb = lw.div_ceil(bw) as usize * bpb as usize;
                    let rws = lh.div_ceil(bh) as usize;
                    let mut v = vec![0u8; rb * rws];
                    for y in 0..rws {
                        let s = y * stride;
                        if s + rb <= storage.len() {
                            v[y * rb..(y + 1) * rb].copy_from_slice(&storage[s..s + rb]);
                        }
                    }
                    (v, rb, rws as u32, 1)
                } else {
                    let Some(g) = deswizzle::geom(&d, level, if d.target == target::T3D { 0 } else { layer }) else { continue };
                    let mut v = vec![0u8; g.row_bytes as usize * g.rows as usize * g.slices as usize];
                    if deswizzle::deswizzle(&d, level, if d.target == target::T3D { 0 } else { layer }, storage, &mut v, g.row_bytes as usize).is_none() {
                        crate::log::red_once(&format!("texture {}x{} format {:#x}: storage shorter than its layout", d.width, d.height, d.format));
                        continue;
                    }
                    (v, g.row_bytes as usize, g.rows, g.slices)
                };
                let _ = (row_bytes, rows);
                if let Some((abw, abh, srgb)) = im.tf.astc {
                    crate::tex::astc::record(self, sub, &rows_data, lw, lh, slices, abw, abh, srgb, im.bc7, im.image, level, layer);
                    continue;
                }
                let data = if im.tf.expand3 != 0 { expand3(&rows_data, im.tf.expand3 as usize) } else { rows_data };
                let (buf, off) = sub.stage_bytes(self, &data);
                copies.push((
                    buf,
                    vk::BufferImageCopy {
                        buffer_offset: off,
                        buffer_row_length: 0,
                        buffer_image_height: 0,
                        image_subresource: vk::ImageSubresourceLayers { aspect_mask: upload_aspect(&im.tf), mip_level: level, base_array_layer: layer, layer_count: 1 },
                        image_offset: vk::Offset3D::default(),
                        image_extent: vk::Extent3D { width: lw, height: lh, depth: if im.ty == vk::ImageType::TYPE_3D { ld } else { 1 } },
                    },
                ));
            }
            if d.target == target::T3D {
                break;
            }
        }
        for (buf, r) in copies {
            unsafe { self.dev().cmd_copy_buffer_to_image(sub.pre, buf, im.image, vk::ImageLayout::GENERAL, &[r]) };
        }
    }

    /// nvnTextureWriteTexels: into the storage (block-linear or linear) and into the image at the next submission.
    pub fn write_texels(&self, t: &tex_t, v: Option<view_t>, r: &NVNcopyRegion, data: *const u8, row: usize, img: usize) {
        let level = v.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
        let layer0 = v.filter(|v| v.bits & VW_LAYERS != 0).map(|v| v.min_layer as u32).unwrap_or(0);
        let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
        let rb = (r.width.max(0) as u32).div_ceil(bw) as usize * bpb as usize;
        let rows = (r.height.max(0) as u32).div_ceil(bh) as usize;
        let depth = r.depth.max(1) as usize;
        let mut imgs = self.images.lock().unwrap();
        if !self.ensure_image(&mut imgs, t) {
            return;
        }
        let is3d = t.d.target == target::T3D;
        for z in 0..depth {
            let mut lin = vec![0u8; rb * rows];
            for y in 0..rows {
                let src = unsafe { data.add(z * img + y * row) };
                unsafe { core::ptr::copy_nonoverlapping(src, lin.as_mut_ptr().add(y * rb), rb) };
            }
            let layer = if is3d { layer0 } else { layer0 + r.zoffset as u32 + z as u32 };
            let zoff = if is3d { r.zoffset + z as i32 } else { 0 };
            imgs.writes.push((
                t.serial,
                level,
                layer,
                vk::Offset3D { x: r.xoffset, y: r.yoffset, z: zoff },
                vk::Extent3D { width: r.width as u32, height: r.height as u32, depth: 1 },
                lin,
            ));
        }
        if let Some(im) = imgs.by_serial.get_mut(&t.serial) {
            if im.tf.expand3 != 0 || im.tf.astc.is_some() {
                crate::log::red_once("WriteTexels into an RGB or ASTC texture: not converted yet");
            }
        }
    }

    /// nvnTextureReadTexels: the image's texels (a GPU copy, waited for).
    pub fn read_texels(&self, t: &tex_t, v: Option<view_t>, r: &NVNcopyRegion, dst: *mut u8, row: usize, img: usize) {
        let level = v.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
        let layer0 = v.filter(|v| v.bits & VW_LAYERS != 0).map(|v| v.min_layer as u32).unwrap_or(0);
        let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
        let rb = (r.width.max(0) as u32).div_ceil(bw) as usize * bpb as usize;
        let rows = (r.height.max(0) as u32).div_ceil(bh) as usize;
        let depth = r.depth.max(1) as usize;
        let total = (rb * rows * depth) as u64;
        let Ok(rbuf) = self.buffer(total, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "read texels") else { return };
        let is3d = t.d.target == target::T3D;
        let f = super::queue::oneoff(self, |g, sub, imgs| {
            if !g.ensure_image(imgs, t) {
                return;
            }
            g.prepare_image(imgs, t.serial, sub);
            let im = &imgs.by_serial[&t.serial];
            let reg = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: upload_aspect(&im.tf),
                    mip_level: level,
                    base_array_layer: if is3d { layer0 } else { layer0 + r.zoffset as u32 },
                    layer_count: if is3d { 1 } else { depth as u32 },
                },
                image_offset: vk::Offset3D { x: r.xoffset, y: r.yoffset, z: if is3d { r.zoffset } else { 0 } },
                image_extent: vk::Extent3D { width: r.width as u32, height: r.height as u32, depth: if is3d { depth as u32 } else { 1 } },
            };
            unsafe { g.dev().cmd_copy_image_to_buffer(sub.main, im.image, vk::ImageLayout::GENERAL, rbuf.buf, &[reg]) };
        });
        super::queue::wait_fence(f, u64::MAX);
        for z in 0..depth {
            for y in 0..rows {
                unsafe { core::ptr::copy_nonoverlapping(rbuf.ptr.add((z * rows + y) * rb), dst.add(z * img + y * row), rb) };
            }
        }
        self.free_buffer(rbuf);
    }
}

/// The aspect a buffer <-> image copy of this format uses (depth for depth-stencil formats).
pub fn upload_aspect(tf: &TexFormat) -> vk::ImageAspectFlags {
    match tf.kind {
        Kind::DepthStencil | Kind::Depth => vk::ImageAspectFlags::DEPTH,
        Kind::Stencil => vk::ImageAspectFlags::STENCIL,
        _ => vk::ImageAspectFlags::COLOR,
    }
}

/// RGB texels as RGBA (alpha 1 in the component's own encoding: the channel is never read, the view maps A to one).
fn expand3(src: &[u8], comp: usize) -> Vec<u8> {
    let px = src.len() / (3 * comp);
    let mut out = vec![0u8; px * 4 * comp];
    for i in 0..px {
        out[i * 4 * comp..i * 4 * comp + 3 * comp].copy_from_slice(&src[i * 3 * comp..(i + 1) * 3 * comp]);
    }
    out
}

/// Which textures are uploaded from their storage, and what that costs (the `[vk] uploads` line every 600 frames).
pub mod upload_stats {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static S: Mutex<Option<HashMap<u64, (String, u64, u64)>>> = Mutex::new(None);

    pub fn note(serial: u64, t: &crate::nvn::obj::tex_t, ns: u64) {
        let mut g = S.lock().unwrap();
        let m = g.get_or_insert_with(HashMap::new);
        let e = m.entry(serial).or_insert_with(|| {
            (format!("ser{serial} {}x{}x{} fmt {:#x} flags {:#x} at {:#x} ({} KB)", t.d.width, t.d.height, t.d.depth, t.d.format, t.d.flags, t.mem as u64, t.l.size / 1024), 0, 0)
        });
        e.1 += 1;
        e.2 += ns;
    }

    /// The most expensive uploads since the last call (the window is cleared).
    pub fn line(frames: u64) -> Option<String> {
        let m = S.lock().unwrap().take()?;
        let total: u64 = m.values().map(|e| e.2).sum();
        let mut v: Vec<_> = m.into_values().collect();
        v.sort_by(|a, b| b.2.cmp(&a.2));
        let f = frames.max(1) as f64;
        let top: Vec<String> = v.iter().take(6).map(|e| format!("{} x{:.2}/frame {:.2} ms/frame", e.0, e.1 as f64 / f, e.2 as f64 / 1e6 / f)).collect();
        Some(format!("uploads from storage: {} textures, {:.2} ms per frame; most: {}", v.len(), total as f64 / 1e6 / f, top.join("; ")))
    }
}

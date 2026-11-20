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
    /// its storage decoded ahead (on several threads) for the upload due
    pub decoded: Option<Vec<Part>>,
    /// a texture in a sparse virtual pool: the physical ranges its last upload read and the mapping generation then
    pub sparse: Option<(Vec<(u64, u64)>, u64)>,
    /// a hash of each 4 KB page of the storage as last uploaded: a CPU write to a page the texture shares with other
    /// data (a uniform ring, a neighbour) that leaves its bytes as they were uploads nothing
    pub page_hash: Vec<u64>,
    /// uploads again because the CPU wrote its pages (its page hashes are kept from the second on)
    pub rewrites: u32,
    /// the resolution scale of this image (a render target's; 1 for everything else): `extent` is the texture's size
    /// times it
    pub scale: f32,
    /// another texture object's image over the same storage (its serial): this one views that image and owns
    /// neither the image nor its memory (Beach Bounce draws its scene into one texture object and samples it through
    /// another over the same storage)
    pub alias_of: Option<u64>,
}
unsafe impl Send for Image {}

const HPAGE: u64 = 4096;

fn hash_bytes(b: &[u8]) -> u64 {
    // four independent lanes of 8 bytes (one lane's multiply chain ran at ~2 GB/s: Violet hashed ~70 MB of
    // re-written texture pages a frame, 17-37 ms)
    let mut l = [0xcbf29ce484222325u64, 0x84222325cbf29ce4, 0x9e3779b97f4a7c15, 0x632be59bd9b4e019];
    let mut ch = b.chunks_exact(32);
    for c in &mut ch {
        for (i, x) in l.iter_mut().enumerate() {
            let v = u64::from_le_bytes(c[8 * i..8 * i + 8].try_into().unwrap());
            *x = (*x ^ v).wrapping_mul(0x100000001b3).rotate_left(23);
        }
    }
    let mut h = l[0] ^ l[1].rotate_left(17) ^ l[2].rotate_left(31) ^ l[3].rotate_left(47);
    for &x in ch.remainder() {
        h = (h ^ x as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// Hashes of the pages of [lo, hi) from `first` (page index from lo's page) on, read in one piece (one VirtualQuery
/// walk: a query a page took minutes in a process with many lazily committed regions).
fn hashes_of(lo: u64, hi: u64, from: u64, to: u64) -> Vec<(usize, u64)> {
    let base = lo & !(HPAGE - 1);
    let (a, e) = ((base + from * HPAGE).max(lo), (base + to * HPAGE).min(hi));
    if e <= a {
        return vec![];
    }
    let bytes = super::mem::guest_bytes(a, e - a);
    let mut out = vec![];
    let mut p = from;
    while p < to {
        let (s0, e0) = ((base + p * HPAGE).max(lo), (base + (p + 1) * HPAGE).min(hi));
        if e0 > s0 && s0 >= a && e0 <= e {
            out.push((p as usize, hash_bytes(&bytes[(s0 - a) as usize..(e0 - a) as usize])));
        }
        p += 1;
    }
    out
}

/// Hashes of every page of a texture's storage.
fn page_hashes(lo: u64, hi: u64) -> Vec<u64> {
    let n = ((hi + HPAGE - 1) / HPAGE - lo / HPAGE) as usize;
    let mut v = vec![0u64; n];
    for (i, h) in hashes_of(lo, hi, 0, n as u64) {
        v[i] = h;
    }
    v
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
    let (p0, p1) = ((s - base) / HPAGE, (e - 1 - base) / HPAGE + 1);
    for (i, h) in hashes_of(lo, hi, p0, p1.min(hashes.len() as u64)) {
        if h != hashes[i] {
            hashes[i] = h;
            changed = true;
        }
    }
    changed
}

/// One level / layer of a texture laid out linearly (as the upload copies it, or as the ASTC encoder takes it).
pub struct Part {
    level: u32,
    layer: u32,
    lw: u32,
    lh: u32,
    ld: u32,
    slices: u32,
    data: Vec<u8>,
}

/// The levels and layers of a texture from its storage bytes, laid out linearly.
fn decode_parts(im: &Image, storage: &[u8]) -> Vec<Part> {
    let t = im.tex;
    let d = t.d;
    let (bpb, bw, bh) = layout::fmt_bpb(d.format);
    let linear = d.flags as u32 & (tex_flags::LINEAR | tex_flags::LINEAR_RENDER_TARGET) != 0;
    let mut out = Vec::new();
    // the texture's own size (a scaled render target's image is larger)
    let native = shape(&t).1;
    for layer in 0..im.layers {
        for level in 0..im.levels {
            let lw = (native.width >> level).max(1);
            let lh = (native.height >> level).max(1);
            let ld = (native.depth >> level).max(1);
            let (rows_data, slices) = if linear {
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
                (v, 1)
            } else {
                let Some(g) = deswizzle::geom(&d, level, if d.target == target::T3D { 0 } else { layer }) else { continue };
                let mut v = vec![0u8; g.row_bytes as usize * g.rows as usize * g.slices as usize];
                if deswizzle::deswizzle(&d, level, if d.target == target::T3D { 0 } else { layer }, storage, &mut v, g.row_bytes as usize).is_none() {
                    crate::log::red_once(&format!("texture {}x{} format {:#x}: storage shorter than its layout", d.width, d.height, d.format));
                    continue;
                }
                (v, g.slices)
            };
            let data = if im.tf.astc.is_none() && im.tf.expand3 != 0 { expand3(&rows_data, im.tf.expand3 as usize) } else { rows_data };
            out.push(Part { level, layer, lw, lh, ld, slices, data });
        }
        if d.target == target::T3D {
            break;
        }
    }
    out
}

/// The storage of the textures among `serials` due an upload (not of sparse pools), decoded on several threads; only
/// when there are enough of them to be worth the threads.
fn decode_many(imgs: &Images, serials: &[u64]) -> HashMap<u64, Vec<Part>> {
    let due: Vec<&Image> = serials
        .iter()
        .filter_map(|s| imgs.by_serial.get(s))
        .filter(|im| im.needs_upload && !im.gpu_owned && !im.tex.mem.is_null() && !crate::nvn::vpool::is_sparse(im.tex.mem as u64))
        .collect();
    let bytes: u64 = due.iter().map(|im| im.tex.l.size).sum();
    if due.len() < 4 || bytes < (4 << 20) {
        return HashMap::new();
    }
    for im in &due {
        super::lazy::touch(im.tex.mem as u64, im.tex.l.size);
    }
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(HashMap::new());
    std::thread::scope(|sc| {
        for _ in 0..n {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(im) = due.get(i) else { break };
                let storage = super::mem::guest_bytes(im.tex.mem as u64, im.tex.l.size);
                let parts = decode_parts(im, &storage);
                out.lock().unwrap().insert(im.tex.serial, parts);
            });
        }
    });
    out.into_inner().unwrap()
}

#[derive(Default)]
pub struct Images {
    pub by_serial: HashMap<u64, Image>,
    /// storage start -> serials (CPU-written textures)
    by_addr: BTreeMap<u64, Vec<u64>>,
    max_size: u64,
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
    // the storage, the description (size, format, target, levels, flags) and the layout; not the swizzle, the pool
    // object or the builder's packaged layout bytes (a view's swizzle is set again on reuse)
    let d = unsafe { core::slice::from_raw_parts(&t.d as *const _ as *const u8, core::mem::size_of_val(&t.d)) };
    let mut b = Vec::with_capacity(d.len() + 24);
    b.extend_from_slice(d);
    b.extend_from_slice(&(t.mem as u64).to_le_bytes());
    b.extend_from_slice(&t.l.size.to_le_bytes());
    b.push(t.bits & 2);
    super::queue::memo_hash(&b)
}

/// Whether a new image takes its content from the texture's storage. Not a compressible texture's (a render target:
/// the GPU draws it, and its storage holds no picture the CPU made; Mario Kart 8 Deluxe's dynamic resolution makes new
/// 1880x1057 .. 1920x1080 targets every frame, each was uploaded from storage: 20-50 ms a frame in races).
fn from_storage(t: &tex_t) -> bool {
    !t.mem.is_null() && t.d.flags as u32 & tex_flags::COMPRESSIBLE == 0
}

const PARK_FRAMES: u64 = 8;
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
        {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            static BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let b = BYTES.fetch_add(t.l.size, std::sync::atomic::Ordering::Relaxed) + t.l.size;
            if t.l.size >= 64 << 20 || n % 2000 == 0 {
                crate::vklog!("image {n} ({} MB made so far): {}x{}x{} format {:#x} target {}, {} MB", b >> 20, t.d.width, t.d.height, t.d.depth, t.d.format, t.d.target, t.l.size >> 20);
            }
        }
        let Some(mut tf) = format::tex_format(t.d.format) else {
            crate::log::red_once(&format!("texture format {:#x} has no Vulkan format", t.d.format));
            return None;
        };
        if t.d.target == target::BUFFER {
            crate::log::red_once("buffer textures are not implemented yet");
            return None;
        }
        let (ty, mut extent, layers) = shape(t);
        let levels = t.d.levels.max(1);
        let scale = super::scale::for_new(t);
        if scale != 1.0 {
            extent.width = super::scale::dim(extent.width, scale);
            extent.height = super::scale::dim(extent.height, scale);
        }
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
        if ty == vk::ImageType::TYPE_3D && usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) {
            flags |= vk::ImageCreateFlags::TYPE_2D_ARRAY_COMPATIBLE;
        }
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
        // a sparse texture of a GPU-only sparse pool: a Vulkan sparse image with memory for its mapped tiles only
        if super::sparse::wanted(self, t, &ci) {
            if let Some(image) = super::sparse::create(self, t, &ci) {
                return Some(Image {
                    tex: *t,
                    image,
                    alloc: None,
                    format: tf.vk,
                    tf,
                    ty,
                    extent,
                    levels,
                    layers,
                    samples,
                    usage,
                    views: HashMap::new(),
                    gpu_owned: true,
                    needs_init: true,
                    needs_upload: false,
                    bc7,
                    decoded: None,
                    sparse: None,
                    page_hash: Vec::new(),
                    rewrites: 0,
                    alias_of: None,
                    scale: 1.0,
                });
            }
        }
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
            needs_upload: from_storage(t),
            bc7,
            decoded: None,
            sparse: None,
            page_hash: Vec::new(),
            rewrites: 0,
            scale,
            alias_of: None,
        })
    }

    /// The image of a texture, created at its first use.
    /// (images, parked images)
    pub fn image_counts(&self) -> (usize, usize) {
        let i = self.images.lock().unwrap();
        (i.by_serial.len(), i.parked_n)
    }

    pub fn ensure_image(&self, imgs: &mut Images, t: &tex_t) -> bool {
        if imgs.by_serial.contains_key(&t.serial) {
            return true;
        }
        // the same texture initialized again over the same memory: its released image, with its content
        let key = tex_identity(t);
        let reused = imgs.parked.get_mut(&key).and_then(|v| v.pop()).map(|(mut im, _)| {
            imgs.parked_n -= 1;
            if im.tex.swz != t.swz || im.tex.bits != t.bits {
                // views carry the swizzle: made again
                let views: Vec<vk::ImageView> = im.views.drain().map(|(_, v)| v).collect();
                self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                    for v in views {
                        g.dev().destroy_image_view(v, None);
                    }
                }));
            }
            im.tex = *t;
            if !im.gpu_owned && im.sparse.is_none() && !im.page_hash.is_empty() && !t.mem.is_null() {
                let lo = t.mem as u64;
                let mut ph = std::mem::take(&mut im.page_hash);
                im.needs_upload |= pages_changed(&mut ph, lo, lo + t.l.size, lo, t.l.size);
                im.page_hash = ph;
            } else if !im.gpu_owned {
                im.needs_upload = from_storage(t);
            }
            super::perf::inc(super::perf::C::ImagesReused);
            im
        });
        // a live image of a texture object with the same storage and description, drawn by the GPU: the new
        // object takes it over (Mario Kart 8 Deluxe initializes new objects over its render targets' storage every
        // frame without finalizing the old ones: each was a new image uploaded from storage, 15-50 ms a frame, and
        // the old ones were never freed)
        let reused = reused.or_else(|| {
            let old = imgs.by_addr.get(&(t.mem as u64))?.iter().rev().copied().find(|s| imgs.by_serial.get(s).is_some_and(|im| im.gpu_owned && tex_identity(&im.tex) == key))?;
            let mut im = imgs.by_serial.remove(&old)?;
            if let Some(v) = imgs.by_addr.get_mut(&(t.mem as u64)) {
                v.retain(|s| *s != old);
            }
            for a in imgs.by_serial.values_mut() {
                if a.alias_of == Some(old) {
                    a.alias_of = Some(t.serial);
                }
            }
            if im.tex.swz != t.swz || im.tex.bits != t.bits {
                let views: Vec<vk::ImageView> = im.views.drain().map(|(_, v)| v).collect();
                self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                    for v in views {
                        g.dev().destroy_image_view(v, None);
                    }
                }));
            }
            im.tex = *t;
            super::perf::inc(super::perf::C::ImagesReused);
            Some(im)
        });
        let reused = reused.or_else(|| self.alias_image(imgs, t));
        let Some(im) = reused.or_else(|| self.create_image(t)) else { return false };
        if !t.mem.is_null() {
            // textures of sparse virtual pools are checked through their pages, not by address (a 5.8 GB terrain
            // array in this map made every CPU write scan every image: Mario Kart World's first frame took 76 s)
            if !crate::nvn::vpool::is_sparse(t.mem as u64) {
                imgs.by_addr.entry(t.mem as u64).or_default().push(t.serial);
                imgs.max_size = imgs.max_size.max(t.l.size);
            }
        }
        imgs.by_serial.insert(t.serial, im);
        true
    }

    /// A new texture object over the storage of a live image of the same shape (size, levels, layers, samples, bytes
    /// per texel; formats the image can be viewed as): an image that views that one. On the GPU both objects name the
    /// same memory, so what is drawn through one is read through the other; separate images left the second one with
    /// the storage's stale bytes (Beach Bounce's final copy to the window read zeros: lane W2).
    fn alias_image(&self, imgs: &Images, t: &tex_t) -> Option<Image> {
        static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *OFF.get_or_init(|| std::env::var("NXGFX_ALIAS").as_deref() == Ok("0")) || t.mem.is_null() || crate::nvn::vpool::is_sparse(t.mem as u64) {
            return None;
        }
        let tf = format::tex_format(t.d.format)?;
        if tf.astc.is_some() || layout::fmt_bpb(t.d.format).1 > 1 {
            return None;
        }
        let (ty, extent, layers) = shape(t);
        let levels = t.d.levels.max(1);
        let samples = samples_of(t.d.samples);
        let bpb = layout::fmt_bpb(t.d.format).0;
        let depthy = |k: Kind| matches!(k, Kind::Depth | Kind::DepthStencil | Kind::Stencil);
        let (os, o) = imgs.by_addr.get(&(t.mem as u64))?.iter().rev().find_map(|s| {
            let o = imgs.by_serial.get(s)?;
            let (_, oext, _) = shape(&o.tex);
            let same_shape = o.ty == ty && oext == extent && o.levels == levels && o.layers == layers && o.samples == samples && o.sparse.is_none() && !o.bc7;
            let same_fmt = o.format == tf.vk
                || !depthy(o.tf.kind) && !depthy(tf.kind) && o.tf.astc.is_none() && layout::fmt_bpb(o.tex.d.format) == (bpb, 1, 1) && o.alias_of.is_none() && o.alloc.is_some();
            let storage_ok = t.d.flags as u32 & tex_flags::IMAGE == 0 || o.usage.contains(vk::ImageUsageFlags::STORAGE);
            (same_shape && same_fmt && storage_ok && !o.needs_init).then_some((*s, o))
        })?;
        let owner = o.alias_of.unwrap_or(os);
        static SAID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
            crate::vklog!(
                "texture ser{} ({}x{} format {:#x}) shares the image of ser{owner} over the same storage {:#x}",
                t.serial,
                t.d.width,
                t.d.height,
                t.d.format,
                t.mem as u64
            );
        }
        Some(Image {
            tex: *t,
            image: o.image,
            alloc: None,
            format: tf.vk,
            tf,
            ty: o.ty,
            extent: o.extent,
            levels: o.levels,
            layers: o.layers,
            samples: o.samples,
            usage: o.usage,
            views: HashMap::new(),
            gpu_owned: o.gpu_owned,
            needs_init: false,
            // the owner's content not uploaded yet: the same storage, uploaded through whichever is used first
            needs_upload: o.needs_upload && !o.gpu_owned,
            bc7: false,
            decoded: None,
            sparse: None,
            page_hash: Vec::new(),
            rewrites: 0,
            scale: o.scale,
            alias_of: Some(owner),
        })
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
                // a 3D target is drawn as a 2D array of its depth slices (the image is 2D-array compatible); a
                // layered pass picks the slice per primitive (a colour grading table drawn one slice per instance)
                ty = vk::ImageViewType::TYPE_2D_ARRAY;
                let depth = (im.extent.depth >> base_level).max(1);
                let (first, n) = match v.filter(|v| v.bits & VW_LAYERS != 0) {
                    Some(v) => (v.min_layer as u32, (v.nlayers as u32).max(1)),
                    None => (0, depth),
                };
                base_layer = first.min(depth - 1);
                nlayers = n.min(depth - base_layer);
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
        let Some(mut im) = imgs.by_serial.remove(&serial) else { return };
        if let Some(v) = imgs.by_addr.get_mut(&(im.tex.mem as u64)) {
            v.retain(|s| *s != serial);
        }
        // an image shared with other texture objects: an alias only drops its views; an owner hands the image and its
        // memory to the first of its aliases
        let aliases: Vec<u64> = imgs.by_serial.iter().filter(|(_, a)| a.alias_of == Some(serial)).map(|(k, _)| *k).collect();
        if im.alias_of.is_some() || !aliases.is_empty() {
            if let Some((&heir, rest)) = aliases.split_first() {
                let a = imgs.by_serial.get_mut(&heir).unwrap();
                a.alias_of = None;
                a.alloc = im.alloc.take();
                for r in rest {
                    imgs.by_serial.get_mut(r).unwrap().alias_of = Some(heir);
                }
            }
            let views: Vec<vk::ImageView> = im.views.drain().map(|(_, v)| v).collect();
            drop(imgs);
            self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                for v in views {
                    g.dev().destroy_image_view(v, None);
                }
            }));
            return;
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

    /// A new resolution scale: every scaled render target is made again at `s` (its levels blitted over, a
    /// multisampled one cleared), parked ones dropped, and the texture pool's descriptors of them written again.
    /// Returns how many were made again. The submission state is held (the start of a submission).
    pub fn rescale_images(&self, sub: &mut Sub, s: f32) -> usize {
        let mut imgs = self.images.lock().unwrap();
        // parked render targets go (a later initialization makes them at the new scale)
        let mut gone = Vec::new();
        for v in imgs.parked.values_mut() {
            let mut i = 0;
            while i < v.len() {
                if super::scale::candidate(&v[i].0.tex) {
                    gone.push(v.remove(i).0);
                } else {
                    i += 1;
                }
            }
        }
        imgs.parked.retain(|_, v| !v.is_empty());
        imgs.parked_n -= gone.len();
        let serials: Vec<u64> = imgs.by_serial.iter().filter(|(_, im)| im.scale != s && super::scale::candidate(&im.tex) && im.sparse.is_none() && im.alias_of.is_none()).map(|(k, _)| *k).collect();
        let mut done = std::collections::HashSet::new();
        for serial in serials {
            let im = imgs.by_serial.get_mut(&serial).unwrap();
            let (_, native, _) = shape(&im.tex);
            let ext = vk::Extent3D { width: super::scale::dim(native.width, s), height: super::scale::dim(native.height, s), depth: native.depth };
            let mut flags = vk::ImageCreateFlags::empty();
            if !matches!(im.tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                flags |= vk::ImageCreateFlags::MUTABLE_FORMAT;
            }
            if ext.width == ext.height && im.layers >= 6 && im.layers % 6 == 0 && im.samples == vk::SampleCountFlags::TYPE_1 {
                flags |= vk::ImageCreateFlags::CUBE_COMPATIBLE;
            }
            let ci = vk::ImageCreateInfo::default()
                .flags(flags)
                .image_type(im.ty)
                .format(im.format)
                .extent(ext)
                .mip_levels(im.levels)
                .array_layers(im.layers)
                .samples(im.samples)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(im.usage)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            let Ok(image) = (unsafe { self.dev().create_image(&ci, None) }) else { continue };
            let req = unsafe { self.dev().get_image_memory_requirements(image) };
            let Ok(a) = self.alloc.lock().unwrap().allocate(&AllocationCreateDesc { name: "texture", requirements: req, location: MemoryLocation::GpuOnly, linear: false, allocation_scheme: AllocationScheme::GpuAllocatorManaged }) else {
                unsafe { self.dev().destroy_image(image, None) };
                continue;
            };
            if unsafe { self.dev().bind_image_memory(image, a.memory(), a.offset()) }.is_err() {
                continue;
            }
            let asp = aspect(&im.tf);
            let range = vk::ImageSubresourceRange { aspect_mask: asp, base_mip_level: 0, level_count: im.levels, base_array_layer: 0, layer_count: im.layers };
            let b = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(image)
                .subresource_range(range)];
            unsafe { self.dev().cmd_pipeline_barrier2(sub.pre, &vk::DependencyInfo::default().image_memory_barriers(&b)) };
            if im.samples == vk::SampleCountFlags::TYPE_1 && !im.needs_init {
                // the content at the new size (blits cannot read multisampled images: those start cleared)
                let flt = super::scale::filter(self, im.format, im.tf.kind);
                for lv in 0..im.levels {
                    let sl = vk::ImageSubresourceLayers { aspect_mask: asp, mip_level: lv, base_array_layer: 0, layer_count: im.layers };
                    let (ow, oh) = ((im.extent.width >> lv).max(1) as i32, (im.extent.height >> lv).max(1) as i32);
                    let (nw, nh) = ((ext.width >> lv).max(1) as i32, (ext.height >> lv).max(1) as i32);
                    let r = vk::ImageBlit {
                        src_subresource: sl,
                        src_offsets: [vk::Offset3D::default(), vk::Offset3D { x: ow, y: oh, z: 1 }],
                        dst_subresource: sl,
                        dst_offsets: [vk::Offset3D::default(), vk::Offset3D { x: nw, y: nh, z: 1 }],
                    };
                    unsafe { self.dev().cmd_blit_image(sub.pre, im.image, vk::ImageLayout::GENERAL, image, vk::ImageLayout::GENERAL, &[r], flt) };
                }
            } else if !im.needs_init {
                if matches!(im.tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                    let v = vk::ClearDepthStencilValue { depth: 0.0, stencil: 0 };
                    unsafe { self.dev().cmd_clear_depth_stencil_image(sub.pre, image, vk::ImageLayout::GENERAL, &v, &[range]) };
                } else {
                    let v = vk::ClearColorValue { uint32: [0; 4] };
                    unsafe { self.dev().cmd_clear_color_image(sub.pre, image, vk::ImageLayout::GENERAL, &v, &[range]) };
                }
            }
            let old = Image {
                tex: im.tex,
                image: std::mem::replace(&mut im.image, image),
                alloc: std::mem::replace(&mut im.alloc, Some(a)),
                format: im.format,
                tf: im.tf,
                ty: im.ty,
                extent: im.extent,
                levels: im.levels,
                layers: im.layers,
                samples: im.samples,
                usage: im.usage,
                views: std::mem::take(&mut im.views),
                gpu_owned: true,
                needs_init: false,
                needs_upload: false,
                bc7: im.bc7,
                decoded: None,
                sparse: None,
                page_hash: Vec::new(),
                rewrites: 0,
                scale: im.scale,
                alias_of: None,
            };
            im.extent = ext;
            im.scale = s;
            gone.push(old);
            done.insert(serial);
        }
        super::queue::full_barrier(self, sub.pre);
        // aliases of the images made again view the new ones
        let followers: Vec<u64> = imgs.by_serial.iter().filter(|(_, a)| a.alias_of.is_some_and(|o| done.contains(&o))).map(|(k, _)| *k).collect();
        for f in followers {
            let o = imgs.by_serial[&imgs.by_serial[&f].alias_of.unwrap()].image;
            let (ext, sc) = { let oi = &imgs.by_serial[&imgs.by_serial[&f].alias_of.unwrap()]; (oi.extent, oi.scale) };
            let a = imgs.by_serial.get_mut(&f).unwrap();
            let views: Vec<vk::ImageView> = a.views.drain().map(|(_, v)| v).collect();
            self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                for v in views {
                    g.dev().destroy_image_view(v, None);
                }
            }));
            a.image = o;
            a.extent = ext;
            a.scale = sc;
            done.insert(f);
        }
        // the texture pool's descriptors of the images made again
        for (id, serial, v) in self.tables.ids_of(&done) {
            if let Some(im) = imgs.by_serial.get_mut(&serial) {
                if let Some(view) = self.image_view(im, v.as_ref(), false) {
                    self.tables.write_image(&self.d, id, view, None);
                }
            }
        }
        drop(imgs);
        for im in gone {
            self.destroy_released(im);
        }
        done.len()
    }

    fn destroy_released(&self, im: Image) {
        {
            self.q.defer_after_use(
                Box::new(move |g: &Gpu| unsafe {
                    for v in im.views.values() {
                        g.dev().destroy_image_view(*v, None);
                    }
                    super::sparse::destroy(im.image);
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
        let Some(im) = imgs.by_serial.get_mut(&serial) else { return };
        im.gpu_owned = true;
        im.needs_upload = false;
        // the texture objects sharing its image
        let (image, at) = (im.image, im.tex.mem as u64);
        let Some(group) = imgs.by_addr.get(&at) else { return };
        if group.len() < 2 {
            return;
        }
        for s in group.clone() {
            if let Some(o) = imgs.by_serial.get_mut(&s) {
                if o.image == image {
                    o.gpu_owned = true;
                    o.needs_upload = false;
                }
            }
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

    /// Descriptor buffers: a buffer texture's texel buffer as (device address, bytes, format), the checks of
    /// `texel_view`.
    pub fn texel_addr(&self, sub: &mut Sub, t: &tex_t, storage: bool) -> Option<vk::DescriptorAddressInfoEXT<'static>> {
        let tf = format::tex_format(t.d.format)?;
        let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
        if bw != 1 || bh != 1 || bpb == 0 || tf.expand3 != 0 {
            return None;
        }
        let need = if storage { vk::FormatFeatureFlags::STORAGE_TEXEL_BUFFER } else { vk::FormatFeatureFlags::UNIFORM_TEXEL_BUFFER };
        let ff = unsafe { self.d.instance.get_physical_device_format_properties(self.d.pdev, tf.vk) }.buffer_features;
        if !ff.contains(need) {
            return None;
        }
        let n = (t.d.width as u64).min(self.d.props.limits.max_texel_buffer_elements as u64) * bpb as u64;
        if n == 0 || t.mem.is_null() {
            return None;
        }
        let (buf, off, _) = super::mem::resolve(self, sub, t.mem as u64, n)?;
        if off % self.d.props.limits.min_texel_buffer_offset_alignment != 0 {
            crate::log::red_once(&format!("buffer texture at an offset Vulkan cannot view ({off:#x})"));
            return None;
        }
        let base = super::mem::buffer_address(buf)?;
        Some(vk::DescriptorAddressInfoEXT::default().address(base + off).range(n).format(tf.vk))
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
            if self.d.caps.descbuf {
                if let Some(u) = self.texel_addr(sub, &t, false) {
                    let s = self.texel_addr(sub, &t, true);
                    self.tables.write_texel_addrs(&self.d, &self.layout, id, u, s);
                }
                continue;
            }
            let Some(uv) = self.texel_view_persistent(sub, &t, false) else { continue };
            let sv = self.texel_view_persistent(sub, &t, true);
            self.tables.write_texel_views(&self.d, id, uv, sv);
        }
        let mut imgs = self.images.lock().unwrap();
        let t_all = std::time::Instant::now();
        if !dirty.is_empty() && !imgs.by_addr.is_empty() {
            let max = imgs.max_size;
            let mut hit = Vec::new();
            let dirty_sorted = dirty.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0);
            let per_range: usize = if dirty_sorted { dirty.len().min(imgs.by_addr.len()) } else { usize::MAX };
            if dirty_sorted && per_range >= 64 {
                // many dirty ranges (a game's loading: Mario Kart World's first submission had ~30,000 textures and
                // pages without end; a lookup per range over the largest texture's span took 17 s): one pass over the
                // textures, each looking its storage up in the sorted ranges
                for ss in imgs.by_addr.values() {
                    for &sv in ss {
                        if let Some(im) = imgs.by_serial.get(&sv) {
                            let (lo, hi) = (im.tex.mem as u64, im.tex.mem as u64 + im.tex.l.size);
                            let i0 = dirty.partition_point(|&(a, n)| a + n <= lo);
                            if i0 < dirty.len() && dirty[i0].0 < hi {
                                hit.push(sv);
                            }
                        }
                    }
                }
            } else {
                for &(a, n) in dirty {
                    for (_, ss) in imgs.by_addr.range(a.saturating_sub(max)..a + n) {
                        hit.extend_from_slice(ss);
                    }
                }
            }
            // once each (a texture under many dirty pages was hashed once per page: Violet's 40 ms of prep a frame)
            hit.sort_unstable();
            hit.dedup();
            for s in hit {
                if let Some(im) = imgs.by_serial.get_mut(&s) {
                    let lo = im.tex.mem as u64;
                    let hi = lo + im.tex.l.size;
                    // the dirty ranges over this texture only
                    let mine: Vec<(u64, u64)> = if dirty_sorted {
                        let i0 = dirty.partition_point(|&(a, n)| a + n <= lo);
                        dirty[i0..].iter().take_while(|&&(a, _)| a < hi).copied().collect()
                    } else {
                        dirty.iter().filter(|&&(a, n)| a < hi && a + n > lo).copied().collect()
                    };
                    let dirty = &mine[..];
                    let covered: u64 = dirty.iter().map(|&(a, n)| (a + n).min(hi).saturating_sub(a.max(lo))).sum();
                    // a GPU-written texture is taken back from its storage only when the CPU rewrote most of it (a
                    // decoded movie frame); a write to a page it merely shares keeps the GPU's content
                    // bytes the texture already holds (a write to a page it shares with other data): nothing to upload
                    if covered > 0 && !im.gpu_owned && !im.needs_upload && im.sparse.is_none() && !im.page_hash.is_empty() {
                        let mut ph = std::mem::take(&mut im.page_hash);
                        let th = std::time::Instant::now();
                        let changed = super::perf::timed(super::perf::C::HashNs, || dirty.iter().fold(false, |c, &(a, n)| pages_changed(&mut ph, lo, hi, a, n) || c));
                        let hms = th.elapsed().as_secs_f64() * 1e3;
                        if hms > 2.0 {
                            static SLOW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                            if SLOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 10 {
                                let dn: u64 = dirty.iter().map(|&(a, n)| (a + n).min(hi).saturating_sub(a.max(lo))).sum();
                                crate::vklog!("texture hash check took {hms:.1} ms: ser{s} {}x{} fmt {:#x} at {lo:#x} ({} KB), {} dirty ranges over it ({} KB), {} page hashes", im.tex.d.width, im.tex.d.height, im.tex.d.format, (hi - lo) >> 10, dirty.len(), dn >> 10, ph.len());
                            }
                        }
                        im.page_hash = ph;
                        if !changed {
                            static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                            if im.rewrites > 30 && im.rewrites < 32 && SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 12 {
                                let d = &im.tex.d;
                                let dn: u64 = dirty.iter().map(|&(a, n)| (a + n).min(hi).saturating_sub(a.max(lo))).sum();
                                crate::vklog!("texture rewritten with the same bytes again and again: ser{s} {}x{}x{} fmt {:#x} flags {:#x} at {lo:#x} ({} KB), {} KB of it written this time in {} ranges", d.width, d.height, d.depth, d.format, d.flags, (hi - lo) >> 10, dn >> 10, dirty.len());
                            }
                            im.rewrites += 1;
                            super::perf::inc(super::perf::C::UploadsSame);
                            continue;
                        }
                    }
                    if covered > 0 && (!im.gpu_owned || covered * 2 >= hi - lo) {
                        im.rewrites += 1;
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
        // the textures' storage read and laid out linearly on several threads first (a streaming burst: Pokemon
        // Legends: Z-A uploads ~1,400 textures in a few seconds, 0.5 ms each on one thread: frames of 200-370 ms)
        let t_scan = t_all.elapsed().as_secs_f64();
        let mut pre = decode_many(&imgs, &serials);
        let t_dec = t_all.elapsed().as_secs_f64();
        let mut t_init = t_dec;
        if !serials.is_empty() {
            // all initializations, one barrier, all uploads, one barrier
            for &s in &serials {
                self.prepare_image_in(&mut imgs, s, sub, 1);
            }
            super::queue::full_barrier(self, sub.pre);
            t_init = t_all.elapsed().as_secs_f64();
            for &s in &serials {
                if let Some(parts) = pre.remove(&s) {
                    if let Some(im) = imgs.by_serial.get_mut(&s) {
                        im.decoded = Some(parts);
                    }
                }
                self.prepare_image_in(&mut imgs, s, sub, 2);
            }
            super::queue::full_barrier(self, sub.pre);
        }
        let t_up = t_all.elapsed().as_secs_f64();
        if t_up > 0.5 {
            crate::vklog!("textures at a submission: {t_up:.2} s for {} images due: CPU writes checked {t_scan:.2} s, decoded ahead {:.2} s, initialized {:.2} s, uploaded {:.2} s", serials.len(), t_dec - t_scan, t_init - t_dec, t_up - t_init);
        }
    }

    /// Layout and contents of an image, in the submission's upload command buffer (also used mid-replay for a render
    /// target created there: the upload command buffer runs before the replay).
    pub fn prepare_image(&self, imgs: &mut Images, serial: u64, sub: &mut Sub) {
        self.prepare_image_in(imgs, serial, sub, 0);
    }

    /// `phase` 0: initialization and upload with their barriers; 1: initialization only, 2: upload only, both without
    /// the barrier after (the caller puts one after a whole batch: Mario Kart World's first submission initialized
    /// ~30,000 images, two full barriers each)
    fn prepare_image_in(&self, imgs: &mut Images, serial: u64, sub: &mut Sub, phase: u8) {
        let Some(im) = imgs.by_serial.get_mut(&serial) else { return };
        let range = vk::ImageSubresourceRange { aspect_mask: aspect(&im.tf), base_mip_level: 0, level_count: im.levels, base_array_layer: 0, layer_count: im.layers };
        if im.needs_init && phase != 2 {
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
                if phase == 0 {
                    super::queue::full_barrier(self, sub.pre);
                }
            }
            im.needs_init = false;
        }
        if phase == 1 {
            return;
        }
        if im.needs_upload && !im.gpu_owned && !im.tex.mem.is_null() {
            let t0 = std::time::Instant::now();
            self.upload_from_storage(im, sub);
            // page hashes only for a texture whose pages the CPU writes again (most are uploaded once)
            if im.sparse.is_none() && im.rewrites > 0 {
                let lo = im.tex.mem as u64;
                im.page_hash = page_hashes(lo, lo + im.tex.l.size);
            }
            let ns = t0.elapsed().as_nanos() as u64;
            super::perf::add(super::perf::C::TexUploadNs, ns);
            upload_stats::note(serial, &im.tex, ns);
            if ns > 500_000_000 {
                crate::vklog!("uploading texture ser{serial} ({}x{}x{} format {:#x}, {} MB) took {:.2} s", im.tex.d.width, im.tex.d.height, im.tex.d.depth, im.tex.d.format, im.tex.l.size >> 20, ns as f64 / 1e9);
            }
            if phase == 0 {
                super::queue::full_barrier(self, sub.pre);
            }
        }
        im.needs_upload = false;
    }

    fn upload_from_storage(&self, im: &mut Image, sub: &mut Sub) {
        let t = im.tex;
        let d = t.d;
        if t.l.size >= 64 << 20 {
            crate::vklog!(
                "uploading a {} MB texture from its storage: {}x{}x{} format {:#x} target {} levels {} at {:#x}{}",
                t.l.size >> 20,
                d.width,
                d.height,
                d.depth,
                d.format,
                d.target,
                d.levels,
                t.mem as u64,
                if crate::nvn::vpool::is_sparse(t.mem as u64) { " (sparse pool)" } else { "" }
            );
        }
        if let Some(parts) = im.decoded.take() {
            self.record_parts(im, sub, parts);
            return;
        }
        // a texture of a sparse virtual pool reads its storage through the pool's page table (unmapped pages as 0)
        let owned;
        let owned_c;
        let storage: &[u8] = if crate::nvn::vpool::is_sparse(t.mem as u64) {
            let mut v = vec![0u8; t.l.size as usize];
            let gen = crate::nvn::vpool::generation();
            let runs = crate::nvn::vpool::copy(t.mem as u64, &mut v).unwrap_or_default();
            im.sparse = Some((runs, gen));
            owned = v;
            &owned
        } else {
            super::lazy::touch(t.mem as u64, t.l.size);
            owned_c = super::mem::guest_bytes(t.mem as u64, t.l.size);
            &owned_c
        };
        let parts = decode_parts(im, storage);
        self.record_parts(im, sub, parts);
    }

    fn record_parts(&self, im: &Image, sub: &mut Sub, parts: Vec<Part>) {
        let mut copies = Vec::new();
        for p in parts {
            if let Some((abw, abh, srgb)) = im.tf.astc {
                crate::tex::astc::record(self, sub, &p.data, p.lw, p.lh, p.slices, abw, abh, srgb, im.bc7, im.image, p.level, p.layer);
                continue;
            }
            let (buf, off) = sub.stage_bytes(self, &p.data);
            copies.push((
                buf,
                vk::BufferImageCopy {
                    buffer_offset: off,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers { aspect_mask: upload_aspect(&im.tf), mip_level: p.level, base_array_layer: p.layer, layer_count: 1 },
                    image_offset: vk::Offset3D::default(),
                    image_extent: vk::Extent3D { width: p.lw, height: p.lh, depth: if im.ty == vk::ImageType::TYPE_3D { p.ld } else { 1 } },
                },
            ));
        }
        for (buf, r) in copies {
            copy_buf_img(self, sub.pre, im, buf, r, true);
        }
    }

    /// nvnTextureWriteTexels: into the storage (block-linear or linear) and into the image at the next submission.
    pub fn write_texels(&self, t: &tex_t, v: Option<view_t>, r: &NVNcopyRegion, data: *const u8, row: usize, img: usize) {
        // Turbo's write_texels: the texels go into the texture's storage and every image over that storage uploads
        // from it again. Written into the image alone they were lost to the next upload from the storage and never
        // reached another texture object over the same memory (Mario Kart 8 Deluxe's mode-select preview: the movie
        // decoder writes its frames into the client's texture, the menu samples the game's own object: black).
        let level = v.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
        let layer0 = v.filter(|v| v.bits & VW_LAYERS != 0).map(|v| v.min_layer as u32).unwrap_or(0);
        if t.mem.is_null() || t.l.size == 0 {
            crate::log::red_once("WriteTexels: the texture has no storage");
            return;
        }
        if write_storage(t, level, layer0, r, data, row, img).is_none() {
            crate::log::red_once(&format!(
                "WriteTexels: region {},{},{} {}x{}x{} of level {level} (layer {layer0}) does not fit texture {}x{}x{} format {:#x} target {} flags {:#x}: not written",
                r.xoffset, r.yoffset, r.zoffset, r.width, r.height, r.depth, t.d.width, t.d.height, t.d.depth, t.d.format, t.d.target, t.d.flags
            ));
            return;
        }
        self.storage_written(t.mem as u64, t.l.size);
    }

    /// The CPU wrote [lo, lo + n) behind the graphics layer's back (WriteTexels, a host decoder): every image over
    /// it uploads from its storage at the next submission, also one the GPU wrote before.
    pub fn storage_written(&self, lo: u64, n: u64) {
        let mut imgs = self.images.lock().unwrap();
        let max = imgs.max_size;
        let hit: Vec<u64> = imgs.by_addr.range(lo.saturating_sub(max)..lo + n).flat_map(|(_, ss)| ss.iter().copied()).collect();
        for s in hit {
            if let Some(im) = imgs.by_serial.get_mut(&s) {
                let (a, b) = (im.tex.mem as u64, im.tex.mem as u64 + im.tex.l.size);
                if a < lo + n && lo < b && im.sparse.is_none() {
                    im.gpu_owned = false;
                    im.needs_upload = true;
                }
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
            copy_buf_img(g, sub.main, im, rbuf.buf, reg, false);
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

/// A buffer <-> image copy (`to_image`: into the image), its region in the texture's texels: through a bounce image
/// of the texture's size when the image is scaled (resolution scale).
pub fn copy_buf_img(g: &Gpu, cb: vk::CommandBuffer, im: &Image, buf: vk::Buffer, r: vk::BufferImageCopy, to_image: bool) {
    let dev = g.dev();
    if im.scale == 1.0 {
        unsafe {
            if to_image {
                dev.cmd_copy_buffer_to_image(cb, buf, im.image, vk::ImageLayout::GENERAL, &[r]);
            } else {
                dev.cmd_copy_image_to_buffer(cb, im.image, vk::ImageLayout::GENERAL, buf, &[r]);
            }
        }
        return;
    }
    let sl = r.image_subresource;
    let e = vk::Extent3D { width: r.image_extent.width, height: r.image_extent.height, depth: 1 };
    let Some(b) = super::scale::bounce(g, im.format, e, sl.layer_count) else {
        crate::log::red_once("resolution scale: no bounce image for a copy of a scaled render target: not copied");
        return;
    };
    let bl = vk::ImageSubresourceLayers { aspect_mask: sl.aspect_mask, mip_level: 0, base_array_layer: 0, layer_count: sl.layer_count };
    let br = vk::BufferImageCopy { image_subresource: bl, image_offset: vk::Offset3D::default(), image_extent: e, ..r };
    let flt = super::scale::filter(g, im.format, im.tf.kind);
    let asp = aspect(&im.tf);
    super::scale::general(g, cb, b, asp, sl.layer_count, true);
    unsafe {
        if to_image {
            dev.cmd_copy_buffer_to_image(cb, buf, b, vk::ImageLayout::GENERAL, &[br]);
            super::scale::general(g, cb, b, asp, sl.layer_count, false);
            super::scale::blit(g, cb, b, 1.0, bl, vk::Offset3D::default(), im.image, im.scale, sl, r.image_offset, e, flt);
        } else {
            super::scale::blit(g, cb, im.image, im.scale, sl, r.image_offset, b, 1.0, bl, vk::Offset3D::default(), e, flt);
            super::scale::general(g, cb, b, asp, sl.layer_count, false);
            dev.cmd_copy_image_to_buffer(cb, b, vk::ImageLayout::GENERAL, buf, &[br]);
        }
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

/// WriteTexels' bytes (rows of `row` bytes, images of `img`) into a texture's storage: block-linear levels through
/// the swizzle (a level-layer the region does not cover whole is read first), linear ones row by row at its stride.
fn write_storage(t: &tex_t, level: u32, layer0: u32, r: &NVNcopyRegion, data: *const u8, row: usize, img: usize) -> Option<()> {
    let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
    if bpb == 0 || r.xoffset < 0 || r.yoffset < 0 || r.zoffset < 0 || r.width <= 0 || r.height <= 0 {
        return None;
    }
    let rb = (r.width as u32).div_ceil(bw) as usize * bpb as usize;
    let rows = (r.height as u32).div_ceil(bh) as usize;
    let (x0, y0) = ((r.xoffset as u32 / bw * bpb) as usize, (r.yoffset as u32 / bh) as usize);
    let depth = r.depth.max(1) as usize;
    let is3d = t.d.target == target::T3D;
    // SAFETY: the texture's storage is guest memory of l.size bytes the game keeps while the texture is live
    let storage = unsafe { core::slice::from_raw_parts_mut(t.mem, t.l.size as usize) };
    if t.d.flags as u32 & tex_flags::LINEAR != 0 {
        let stride = if t.d.stride != 0 { t.d.stride as usize } else { (t.d.width.div_ceil(bw) * bpb) as usize };
        if level != 0 || depth != 1 || x0 + rb > stride {
            return None;
        }
        let base = layer0 as usize * t.l.layer_size as usize;
        for y in 0..rows {
            let o = base + (y0 + y) * stride + x0;
            if o + rb > storage.len() {
                return None;
            }
            storage[o..o + rb].copy_from_slice(unsafe { core::slice::from_raw_parts(data.add(y * row), rb) });
        }
        return Some(());
    }
    // block-linear: one level-layer (3D: all its slices) at a time
    let groups: Vec<(u32, Vec<usize>)> = if is3d { vec![(layer0, (0..depth).collect())] } else { (0..depth).map(|z| (layer0 + r.zoffset as u32 + z as u32, vec![z])).collect() };
    for (layer, zs) in groups {
        let g = deswizzle::geom(&t.d, level, layer)?;
        let pitch = g.row_bytes as usize;
        if x0 + rb > pitch || y0 + rows > g.rows as usize {
            return None;
        }
        let z_first = if is3d { r.zoffset as usize } else { 0 };
        if z_first + zs.len() > g.slices as usize {
            return None;
        }
        let mut lin = vec![0u8; pitch * g.rows as usize * g.slices as usize];
        let whole = x0 == 0 && rb == pitch && y0 == 0 && rows == g.rows as usize && zs.len() == g.slices as usize;
        if !whole {
            deswizzle::deswizzle(&t.d, level, layer, storage, &mut lin, pitch)?;
        }
        for &z in &zs {
            let slice = if is3d { z_first + z } else { 0 };
            for y in 0..rows {
                let o = (slice * g.rows as usize + y0 + y) * pitch + x0;
                lin[o..o + rb].copy_from_slice(unsafe { core::slice::from_raw_parts(data.add(z * img + y * row), rb) });
            }
        }
        deswizzle::swizzle(&t.d, level, layer, &lin, pitch, storage)?;
    }
    Some(())
}

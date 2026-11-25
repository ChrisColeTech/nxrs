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
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub struct ViewKey {
    pub view: view_t,
    pub has: bool,
    /// an attachment view: identity swizzle, every aspect
    pub attach: bool,
    /// a storage-image view: identity swizzle (storage descriptors take no swizzle)
    pub storage: bool,
}

/// Diagnostic evidence copied from the final successful vkCreateImageView input, after all format
/// fallbacks and subresource adjustments. Never reconstruct this from the registered ViewKey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedView {
    pub image: u64,
    pub view: u64,
    pub format: i32,
    pub view_type: i32,
    pub aspect: u32,
    pub base_level: u32,
    pub levels: u32,
    pub base_layer: u32,
    pub layers: u32,
    pub components: [i32; 4],
}

impl ResolvedView {
    pub(crate) fn from_create_info(view: vk::ImageView, ci: &vk::ImageViewCreateInfo<'_>) -> Self {
        Self {
            image: vk::Handle::as_raw(ci.image),
            view: vk::Handle::as_raw(view),
            format: ci.format.as_raw(),
            view_type: ci.view_type.as_raw(),
            aspect: ci.subresource_range.aspect_mask.as_raw(),
            base_level: ci.subresource_range.base_mip_level,
            levels: ci.subresource_range.level_count,
            base_layer: ci.subresource_range.base_array_layer,
            layers: ci.subresource_range.layer_count,
            components: [
                ci.components.r,
                ci.components.g,
                ci.components.b,
                ci.components.a,
            ]
            .map(|c| c.as_raw()),
        }
    }
}

#[derive(Default)]
struct ViewEvidence(std::collections::VecDeque<ResolvedView>);

impl ViewEvidence {
    fn record(&mut self, evidence: ResolvedView, limit: usize) {
        // Reused handles replace old evidence even when their image changed.
        self.0.retain(|e| e.view != evidence.view);
        if limit == 0 {
            return;
        }
        while self.0.len() >= limit {
            self.0.pop_front();
        }
        self.0.push_back(evidence);
    }

    fn get(&self, image: u64, view: u64) -> Option<ResolvedView> {
        self.0
            .iter()
            .rev()
            .find(|e| view != 0 && e.view == view && e.image == image)
            .copied()
    }
}

const VIEW_EVIDENCE_LIMIT: usize = 4096;
static VIEW_EVIDENCE: std::sync::OnceLock<Mutex<ViewEvidence>> = std::sync::OnceLock::new();

fn chain_evidence_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_RENDER_CHAIN").as_deref() == Ok("1"))
}

fn note_view(view: vk::ImageView, ci: &vk::ImageViewCreateInfo<'_>) {
    if !chain_evidence_on() {
        return;
    }
    VIEW_EVIDENCE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .record(
            ResolvedView::from_create_info(view, ci),
            VIEW_EVIDENCE_LIMIT,
        );
}

/// Immutable physical creation input, independent of the texture object currently owning it.
/// A new creation receives a new identity even if Vulkan recycles the same numeric image handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageCreation {
    pub id: u64,
    pub image: u64,
    pub format: i32,
    pub extent: [u32; 3],
    pub image_type: i32,
    pub levels: u32,
    pub layers: u32,
    pub samples: u32,
}

impl ImageCreation {
    pub(crate) fn from_create_info(image: vk::Image, ci: &vk::ImageCreateInfo<'_>) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            image: vk::Handle::as_raw(image),
            format: ci.format.as_raw(),
            extent: [ci.extent.width, ci.extent.height, ci.extent.depth],
            image_type: ci.image_type.as_raw(),
            levels: ci.mip_levels,
            layers: ci.array_layers,
            samples: ci.samples.as_raw(),
        }
    }

    fn for_image(self, image: vk::Image) -> Option<Self> {
        (image != vk::Image::null() && self.image == vk::Handle::as_raw(image)).then_some(self)
    }
}

fn note_creation(image: vk::Image, ci: &vk::ImageCreateInfo<'_>) -> Option<ImageCreation> {
    Some(ImageCreation::from_create_info(image, ci))
}

/// Only a current member of this image's view cache can use retained final-create evidence.
/// Missing/evicted evidence stays unknown; this accessor makes no Vulkan capability queries.
pub(crate) fn resolved_view(im: &Image, view: vk::ImageView) -> Option<ResolvedView> {
    if !im.views.values().any(|&v| v == view) {
        return None;
    }
    VIEW_EVIDENCE
        .get()?
        .lock()
        .unwrap()
        .get(vk::Handle::as_raw(im.image), vk::Handle::as_raw(view))
}

#[cfg(test)]
mod view_evidence_tests {
    use super::*;

    #[test]
    fn final_view_metadata_is_bounded_and_reused_handles_do_not_recover_old_interpretations() {
        let ci = vk::ImageViewCreateInfo::default()
            .image(<vk::Image as vk::Handle>::from_raw(100))
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .components(vk::ComponentMapping::default().r(vk::ComponentSwizzle::B))
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        let mut cache = ViewEvidence::default();
        for handle in 1..=3 {
            let evidence = ResolvedView::from_create_info(
                <vk::ImageView as vk::Handle>::from_raw(handle),
                &ci,
            );
            cache.record(evidence, 2);
        }
        assert_eq!(cache.0.len(), 2);
        assert_eq!(cache.get(100, 1), None); // The renderer may still cache this handle; evidence is gone.
        let kept = cache.get(100, 2).unwrap();
        assert_eq!(kept.format, 37);
        assert_eq!(kept.components[0], vk::ComponentSwizzle::B.as_raw());
        assert_eq!(
            (
                kept.aspect,
                kept.base_level,
                kept.levels,
                kept.base_layer,
                kept.layers
            ),
            (1, 0, 1, 0, 1)
        );

        let replacement = ci
            .image(<vk::Image as vk::Handle>::from_raw(200))
            .format(vk::Format::B8G8R8A8_UNORM);
        cache.record(
            ResolvedView::from_create_info(
                <vk::ImageView as vk::Handle>::from_raw(2),
                &replacement,
            ),
            2,
        );
        assert_eq!(cache.0.len(), 2);
        assert_eq!(cache.get(100, 2), None);
        assert_eq!(cache.get(200, 2).unwrap().format, 44);
        assert_eq!(cache.get(200, 0), None);
    }
}

pub struct Image {
    pub tex: tex_t,
    pub image: vk::Image,
    creation: Option<ImageCreation>,
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
    /// Actual successful view inputs, owned by this backing rather than an evictable diagnostic ring.
    view_metadata: HashMap<ViewKey, ResolvedView>,
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
    /// page_hash was taken this submission before the upload's storage read (image_begin_submit)
    pub hashed_early: bool,
    /// the resolution scale of this image (a render target's; 1 for everything else): `extent` is the texture's size
    /// times it
    pub scale: f32,
    /// another texture object's image over the same storage (its serial): this one views that image and owns
    /// neither the image nor its memory (Beach Bounce draws its scene into one texture object and samples it through
    /// another over the same storage)
    pub alias_of: Option<u64>,
    /// counts the GPU writes of the texture (a write-back lands only if no later GPU write came)
    pub gpu_seq: u64,
}
unsafe impl Send for Image {}

impl Image {
    pub(crate) fn prepared_view(&self, key: &ViewKey) -> Option<ResolvedView> {
        let view = self.view_metadata.get(key)?;
        (self.views.get(key).copied().map(vk::Handle::as_raw) == Some(view.view)
            && view.image == vk::Handle::as_raw(self.image)).then_some(*view)
    }

    pub(crate) fn backing_creation(&self) -> Option<ImageCreation> {
        self.creation.and_then(|c| c.for_image(self.image))
    }

    /// Same physical image, new texture owner. Preserve the heir's registered/rendering format.
    fn promote_from(&mut self, former: &mut Image) {
        self.alias_of = None;
        self.alloc = former.alloc.take();
        self.creation = former
            .backing_creation()
            .and_then(|c| c.for_image(self.image));
    }

    /// Every physical replacement (and alias following it) explicitly supplies its new provenance.
    /// Unknown or mismatched creation input clears old evidence instead of inheriting an old label.
    fn replace_backing(
        &mut self,
        image: vk::Image,
        creation: Option<ImageCreation>,
    ) -> (vk::Image, Option<ImageCreation>) {
        let old = (self.image, self.creation);
        self.image = image;
        self.view_metadata.clear();
        self.creation = creation.and_then(|c| c.for_image(image));
        old
    }
}

#[cfg(test)]
mod creation_provenance_tests {
    use super::*;
    use crate::vk::exec::render_chain::{resolve_images, Point};

    fn creation(handle: u64, format: vk::Format) -> ImageCreation {
        let ci = vk::ImageCreateInfo::default()
            .flags(vk::ImageCreateFlags::MUTABLE_FORMAT)
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: 1440,
                height: 810,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1);
        ImageCreation::from_create_info(<vk::Image as vk::Handle>::from_raw(handle), &ci)
    }

    // CPU-only fixture: no allocation, Vulkan device, or handles with a destructor.
    fn texture(serial: u64, registered: vk::Format, c: ImageCreation) -> Image {
        let mut tex: tex_t = unsafe { core::mem::zeroed() };
        tex.serial = serial;
        tex.d.width = 1440;
        tex.d.height = 810;
        tex.d.depth = 1;
        tex.d.levels = 1;
        tex.d.target = target::T2D;
        tex.d.format = if registered == vk::Format::B8G8R8A8_UNORM {
            154
        } else {
            37
        };
        Image {
            tex,
            image: <vk::Image as vk::Handle>::from_raw(c.image),
            creation: Some(c),
            alloc: None,
            format: registered,
            tf: format::tex_format(tex.d.format).unwrap(),
            ty: vk::ImageType::TYPE_2D,
            extent: vk::Extent3D {
                width: 1440,
                height: 810,
                depth: 1,
            },
            levels: 1,
            layers: 1,
            samples: vk::SampleCountFlags::TYPE_1,
            usage: vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT,
            views: HashMap::new(),
            view_metadata: HashMap::new(),
            gpu_owned: true,
            needs_init: false,
            needs_upload: false,
            bc7: false,
            decoded: None,
            sparse: None,
            page_hash: Vec::new(),
            rewrites: 0,
            hashed_early: false,
            scale: 1.0,
            alias_of: None,
            gpu_seq: 4,
        }
    }

    fn sampled_view(im: &Image, handle: u64) -> ResolvedView {
        let ci = vk::ImageViewCreateInfo::default()
            .image(im.image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(im.format)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        ResolvedView::from_create_info(<vk::ImageView as vk::Handle>::from_raw(handle), &ci)
    }

    #[test]
    fn canonical_raw_format_survives_alias_owner_handoff() {
        for registered in [vk::Format::B8G8R8A8_UNORM, vk::Format::R8G8B8A8_UNORM] {
            let c = creation(100, vk::Format::R8G8B8A8_UNORM);
            let owner = texture(7, vk::Format::R8G8B8A8_UNORM, c);
            // Same provenance source as alias_image, not a new creation inferred from alias format.
            let mut alias = texture(8, registered, owner.backing_creation().unwrap());
            alias.alias_of = Some(7);
            let v = sampled_view(&alias, 501);
            let key = ViewKey {
                view: view_t::default(),
                has: false,
                attach: false,
                storage: false,
            };
            alias
                .views
                .insert(key, <vk::ImageView as vk::Handle>::from_raw(v.view));
            let mut images = HashMap::from([(7, owner), (8, alias)]);
            // The exact production transition after tex_release_now removes the old owner.
            let mut former = images.remove(&7).unwrap();
            let heir = images.get_mut(&8).unwrap();
            heir.promote_from(&mut former);
            drop(former);
            assert_eq!(heir.alias_of, None);
            assert_eq!(heir.image, <vk::Image as vk::Handle>::from_raw(100));
            assert_eq!(heir.format, registered);
            assert_eq!(heir.tf.vk, registered);
            assert_eq!(heir.backing_creation(), Some(c));
            assert_eq!(
                heir.views[&key],
                <vk::ImageView as vk::Handle>::from_raw(501)
            );

            // Same helper as chain_resource: backing is resolved from the promoted Image itself.
            let r = resolve_images(heir, heir, v.view, Some(v));
            assert_eq!(r.owner, 8);
            assert_eq!(r.image, 100);
            assert_eq!(r.creation_id, c.id);
            assert_eq!(r.raw_format, 37);
            assert_eq!(r.registered_format, registered.as_raw());
            assert_eq!(r.interpretation.unwrap().format, registered.as_raw());
            assert_eq!(r.bpp, 4);
            assert!(r
                .raw_filename(
                    Point {
                        frame: 55,
                        submission: 12,
                        pass: 18
                    },
                    0
                )
                .ends_with("-fmt37.bin"));
        }
    }

    #[test]
    fn production_metadata_cannot_survive_recycled_backing_or_view() {
        let format = vk::Format::R8G8B8A8_UNORM;
        let c = creation(100, format);
        let mut im = texture(7, format, c);
        let key = ViewKey { view: view_t::default(), has: false, attach: false, storage: false };
        let mut evidence = sampled_view(&im, 501);
        evidence.base_level = 1;
        evidence.base_layer = 2;
        im.views.insert(key, <vk::ImageView as vk::Handle>::from_raw(501));
        im.view_metadata.insert(key, evidence);
        assert_eq!(im.prepared_view(&key), Some(evidence));
        im.views.insert(key, <vk::ImageView as vk::Handle>::from_raw(502));
        assert!(im.prepared_view(&key).is_none());
        let next = creation(100, format); // same numeric VkImage, different creation
        assert_ne!(c.id, next.id);
        im.replace_backing(im.image, Some(next));
        assert!(im.prepared_view(&key).is_none());
    }

    #[test]
    fn new_backing_and_alias_followers_replace_or_clear_creation_provenance() {
        let first = creation(100, vk::Format::R8G8B8A8_UNORM);
        let mut owner = texture(8, vk::Format::B8G8R8A8_UNORM, first);
        let mut alias = texture(
            9,
            vk::Format::R8G8B8A8_UNORM,
            owner.backing_creation().unwrap(),
        );
        alias.alias_of = Some(8);
        let old_view = sampled_view(&owner, 501);
        let next = creation(200, vk::Format::B8G8R8A8_UNORM);
        // Used by rescale/storage replacement and both alias-follower paths.
        let old = owner.replace_backing(<vk::Image as vk::Handle>::from_raw(200), Some(next));
        assert_eq!(old.1, Some(first));
        assert_ne!(next.id, first.id);
        alias.replace_backing(owner.image, owner.backing_creation());
        assert_eq!(alias.backing_creation(), Some(next));
        assert_eq!(alias.format, vk::Format::R8G8B8A8_UNORM);
        let stale = resolve_images(&owner, &owner, old_view.view, Some(old_view));
        assert_eq!((stale.image, stale.bpp), (0, 0));
        let current_view = sampled_view(&owner, 502);
        let before_reuse = resolve_images(&owner, &owner, 502, Some(current_view));
        assert_eq!(before_reuse.raw_format, 44);
        let alias_view = sampled_view(&alias, 503);
        let follower = resolve_images(&owner, &alias, 503, Some(alias_view));
        assert_eq!(follower.raw_format, 44);
        assert_eq!(follower.registered_format, 37);
        assert_eq!(follower.creation_id, next.id);
        assert!(follower.same_image(before_reuse));

        // Even the same numeric H AND format are a different physical creation, not an old observation.
        let recycled = creation(200, vk::Format::B8G8R8A8_UNORM);
        owner.replace_backing(owner.image, Some(recycled));
        let fresh_view = sampled_view(&owner, 504);
        let after_reuse = resolve_images(&owner, &owner, 504, Some(fresh_view));
        assert_eq!(after_reuse.raw_format, 44);
        assert_ne!(after_reuse.creation_id, before_reuse.creation_id);
        assert!(!after_reuse.same_image(before_reuse));
        assert_eq!(owner.format, vk::Format::B8G8R8A8_UNORM);

        // Uninstrumented creation or a mismatched record must not inherit any previous raw label.
        owner.replace_backing(<vk::Image as vk::Handle>::from_raw(300), Some(first));
        assert_eq!(owner.backing_creation(), None);
        let untracked_view = sampled_view(&owner, 505);
        let unknown = resolve_images(&owner, &owner, 505, Some(untracked_view));
        assert_eq!(
            (
                unknown.image,
                unknown.bpp,
                unknown.raw_format,
                unknown.creation_id
            ),
            (0, 0, 0, 0)
        );
        owner.replace_backing(owner.image, None);
        assert_eq!(owner.backing_creation(), None);
    }
}

const HPAGE: u64 = 4096;

/// Image use without NVN's IMAGE flag (Turbo caa07ad). A game may register or bind a texture made without
/// `NVN_TEXTURE_FLAGS_IMAGE` as an image (Ocarina of Time's movie network writes its pictures by image store into
/// RGBA8 1920x1080 and RGBA8_SRGB 2560x1440 textures made with flags 0); the console takes that. Here such an image had
/// no storage use and the writes were lost. On by default (`NXGFX_IMAGE_NOFLAG=0` or `--image_noflag=0` disables it),
/// the storage of such a texture is remembered and every image over it gets storage use, an sRGB
/// one through its UNORM twin (Vulkan has no sRGB storage images; `EXTENDED_USAGE`). An image made before the texture
/// was used so is made again with its content (`upgrade_storage`).
static IMG_MEM: std::sync::Mutex<Option<std::collections::HashSet<u64>>> =
    std::sync::Mutex::new(None);
/// storage marked since the last submission (images over it are made again at its start)
static IMG_PENDING: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

/// A texture write-back's bytes, completed, waiting for the next submission's start (`tex_writeback`).
struct WbLanding {
    t: tex_t,
    serial: u64,
    seq: u64,
    image: vk::Image,
    bpb: u32,
    data: Vec<u8>,
}
unsafe impl Send for WbLanding {}
static WB_LANDING: std::sync::Mutex<Vec<WbLanding>> = std::sync::Mutex::new(Vec::new());

/// Read at each use (a texture registered or bound as an image without the flag; rare), so it can change live.
/// NXGFX_TAKEOVER_FORWARD=0: a texture object whose image another took over has none (as before).
fn forward_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_TAKEOVER_FORWARD").as_deref() != Ok("0"))
}

pub fn image_noflag_on() -> bool {
    match std::env::var("NXGFX_IMAGE_NOFLAG") {
        Ok(v) => v != "0",
        Err(_) => nxconfig::flag("image_noflag"),
    }
}

fn img_wanted(t: &tex_t) -> bool {
    !t.mem.is_null()
        && IMG_MEM
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|m| m.contains(&(t.mem as u64)))
}

/// The UNORM twin of an sRGB format (what a storage view of an sRGB image is made in).
pub fn unorm_twin(f: vk::Format) -> Option<vk::Format> {
    Some(match f {
        vk::Format::R8G8B8A8_SRGB => vk::Format::R8G8B8A8_UNORM,
        vk::Format::B8G8R8A8_SRGB => vk::Format::B8G8R8A8_UNORM,
        vk::Format::A8B8G8R8_SRGB_PACK32 => vk::Format::A8B8G8R8_UNORM_PACK32,
        vk::Format::R8_SRGB => vk::Format::R8_UNORM,
        vk::Format::R8G8_SRGB => vk::Format::R8G8_UNORM,
        _ => return None,
    })
}

/// A texture used as an image without the IMAGE flag: its storage is marked (when `image_noflag` is on); true when it
/// is new to the set, so images over it are made again with storage use (at the next submission's start).
pub fn want_image(t: &tex_t) -> bool {
    if !image_noflag_on()
        || t.mem.is_null()
        || t.d.flags as u32 & tex_flags::IMAGE != 0
        || t.d.target == target::BUFFER
        || layout::fmt_bpb(t.d.format).1 > 1
    {
        return false;
    }
    let Some(tf) = format::tex_format(t.d.format) else {
        return false;
    };
    if tf.astc.is_some() || matches!(tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
        return false;
    }
    let fresh = IMG_MEM
        .lock()
        .unwrap()
        .get_or_insert_with(Default::default)
        .insert(t.mem as u64);
    if fresh {
        IMG_PENDING.lock().unwrap().push(t.mem as u64);
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
            crate::vklog!(
                "texture ser{} {}x{} format {:#x} flags {:#x} is used as an image without the IMAGE flag: its image gets storage use",
                t.serial,
                t.d.width,
                t.d.height,
                t.d.format,
                t.d.flags
            );
        }
    }
    fresh
}

fn hash_bytes(b: &[u8]) -> u64 {
    // four independent lanes of 8 bytes (one lane's multiply chain ran at ~2 GB/s: Violet hashed ~70 MB of
    // re-written texture pages a frame, 17-37 ms)
    let mut l = [
        0xcbf29ce484222325u64,
        0x84222325cbf29ce4,
        0x9e3779b97f4a7c15,
        0x632be59bd9b4e019,
    ];
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
            out.push((
                p as usize,
                hash_bytes(&bytes[(s0 - a) as usize..(e0 - a) as usize]),
            ));
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

/// Bytes covered by dirty pages whose texture bytes actually changed. A first read of demand-zero guest pages
/// can dirty Windows' write watch; those pages are not a CPU rewrite of a GPU-owned image. Updating the hashes
/// also counts overlapping dirty ranges only once. Coverage stays page-granular, as in the write-watch policy.
fn changed_page_coverage(hashes: &mut [u64], lo: u64, hi: u64, dirty: &[(u64, u64)]) -> u64 {
    let base = lo & !(HPAGE - 1);
    let mut covered = 0;
    for &(a, n) in dirty {
        let (s, e) = (a.max(lo), (a + n).min(hi));
        if e <= s {
            continue;
        }
        let (p0, p1) = ((s - base) / HPAGE, (e - 1 - base) / HPAGE + 1);
        for (i, h) in hashes_of(lo, hi, p0, p1.min(hashes.len() as u64)) {
            if hashes[i] != h {
                hashes[i] = h;
                covered +=
                    (base + (i as u64 + 1) * HPAGE).min(hi) - (base + i as u64 * HPAGE).max(lo);
            }
        }
    }
    covered
}

/// CPU-uploaded images that can later become GPU-owned need a baseline from their first upload, not just from
/// their first CPU rewrite. Compressed sampled textures retain the existing rewrite-only hashing policy.
fn hash_upload(im: &Image) -> bool {
    im.rewrites > 0
        || (im.tf.astc.is_none()
            && layout::fmt_bpb(im.tex.d.format).1 == 1
            && im.usage.intersects(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                    | vk::ImageUsageFlags::STORAGE,
            ))
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
/// Bytes of texture storage deswizzled on the GPU.
pub static GPU_DESWIZZLED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether a texture's upload from its storage goes through the GPU deswizzle: block-linear, not ASTC (its blocks are
/// decoded on the GPU from CPU-deswizzled rows), no 3-byte expansion, big enough, every level in whole words.
fn gpu_deswizzle_ok(im: &Image) -> bool {
    let t = im.tex;
    let linear = t.d.flags as u32 & (tex_flags::LINEAR | tex_flags::LINEAR_RENDER_TARGET) != 0;
    !linear
        && im.tf.astc.is_none()
        && im.tf.expand3 == 0
        && t.l.size >= crate::tex::deswizzle_gpu::MIN_BYTES
        && t.l.size < (1 << 31)
        && crate::tex::deswizzle_gpu::fits(
            &t.d,
            if t.d.target == target::T3D {
                1
            } else {
                im.layers
            },
            im.levels,
        )
}

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
                let stride = if d.stride != 0 {
                    d.stride as usize
                } else {
                    (t.l.layer_size / lh.max(1) as u64) as usize
                };
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
                let Some(g) =
                    deswizzle::geom(&d, level, if d.target == target::T3D { 0 } else { layer })
                else {
                    continue;
                };
                let mut v = vec![0u8; g.row_bytes as usize * g.rows as usize * g.slices as usize];
                if deswizzle::deswizzle(
                    &d,
                    level,
                    if d.target == target::T3D { 0 } else { layer },
                    storage,
                    &mut v,
                    g.row_bytes as usize,
                )
                .is_none()
                {
                    crate::log::red_once(&format!(
                        "texture {}x{} format {:#x}: storage shorter than its layout",
                        d.width, d.height, d.format
                    ));
                    continue;
                }
                (v, g.slices)
            };
            let data = if im.tf.astc.is_none() && im.tf.expand3 != 0 {
                expand3(&rows_data, im.tf.expand3 as usize)
            } else {
                rows_data
            };
            out.push(Part {
                level,
                layer,
                lw,
                lh,
                ld,
                slices,
                data,
            });
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
        .filter(|im| {
            im.needs_upload
                && !im.gpu_owned
                && !im.tex.mem.is_null()
                && !crate::nvn::vpool::is_sparse(im.tex.mem as u64)
                && !gpu_deswizzle_ok(im)
        })
        .collect();
    let bytes: u64 = due.iter().map(|im| im.tex.l.size).sum();
    if due.len() < 4 || bytes < (4 << 20) {
        return HashMap::new();
    }
    for im in &due {
        super::lazy::touch(im.tex.mem as u64, im.tex.l.size);
    }
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8);
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
    /// a texture object whose image a newer object over the same storage took over -> that object: the older one is
    /// still the game's (Mario Kart 8 Deluxe registers its scene target, ser6754, into new texture pool slots every
    /// frame while it also makes new objects over the same storage; once one of those took the image, every slot
    /// naming ser6754 bound the zero stand-in: the black stretches of its laps, async replay only by timing)
    forward: HashMap<u64, u64>,
    /// released texture objects (what pool slots may still name): on the console a slot's descriptor keeps naming the
    /// memory, so it shows what the live object over the same storage holds (MK8D finalizes its scene target's object,
    /// ser6754, while slots registered with it are still sampled: 88 stand-in binds a lap, the black stretches)
    released: HashMap<u64, tex_t>,
}

impl Images {
    /// The render targets (colour and depth attachments) as lines for a diagnostic dump: serial, size, layers,
    /// levels, samples, Vulkan and NVN formats, scale, storage address, largest first.
    pub fn render_targets(&self) -> Vec<String> {
        let mut v: Vec<&Image> = self
            .by_serial
            .values()
            .filter(|im| {
                im.usage.intersects(
                    vk::ImageUsageFlags::COLOR_ATTACHMENT
                        | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                )
            })
            .collect();
        v.sort_by_key(|im| {
            (
                std::cmp::Reverse(im.extent.width as u64 * im.extent.height as u64),
                im.tex.serial,
            )
        });
        v.iter()
            .map(|im| {
                format!(
                    "ser{}\t{}x{}x{}\tlayers {}\tlevels {}\tsamples {}\t{:?}\tNVN {:#x}\tscale {}\tat {:#x}{}",
                    im.tex.serial,
                    im.extent.width,
                    im.extent.height,
                    im.extent.depth,
                    im.layers,
                    im.levels,
                    im.samples.as_raw(),
                    im.format,
                    im.tex.d.format,
                    im.scale,
                    im.tex.mem as u64,
                    if im.gpu_owned { "\tGPU-written" } else { "" }
                )
            })
            .collect()
    }
}

/// What a texture object is, without its serial: the key a released image is found again by.
fn tex_identity(t: &tex_t) -> u128 {
    // the storage, the description (size, format, target, levels, flags) and the layout; not the swizzle, the pool
    // object or the builder's packaged layout bytes (a view's swizzle is set again on reuse)
    let d = unsafe {
        core::slice::from_raw_parts(&t.d as *const _ as *const u8, core::mem::size_of_val(&t.d))
    };
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

    /// Where texture object `serial`'s image is (diagnostics): live, parked after its release, or gone.
    pub fn whereabouts(&self, serial: u64) -> &'static str {
        if self.by_serial.contains_key(&serial) {
            "live"
        } else if self
            .parked
            .values()
            .any(|v| v.iter().any(|(im, _)| im.tex.serial == serial))
        {
            if self.live(serial) != serial {
                "released, its image parked; a live object over its storage is used"
            } else {
                "released, its image parked"
            }
        } else if self.forward.contains_key(&serial) {
            "forwarded to an object that is gone"
        } else {
            "released or never made"
        }
    }

    /// The serial whose image texture object `serial` now views: itself, or the object that took its image over
    /// (followed through later take-overs).
    pub fn live(&self, serial: u64) -> u64 {
        let mut s = serial;
        for _ in 0..16 {
            if self.by_serial.contains_key(&s) {
                return s;
            }
            match self.forward.get(&s) {
                Some(&n) => s = n,
                None => break,
            }
        }
        if self.by_serial.contains_key(&s) {
            return s;
        }
        // released: the newest live object over the same storage with the same description (the memory the slot names)
        if let Some(t) = self.released.get(&serial).or_else(|| self.released.get(&s)) {
            if forward_on() && !t.mem.is_null() {
                let key = tex_identity(t);
                let at = self.by_addr.get(&(t.mem as u64));
                // the same description over the same storage
                let same = at.and_then(|v| {
                    v.iter().rev().copied().find(|x| {
                        self.by_serial
                            .get(x)
                            .is_some_and(|im| tex_identity(&im.tex) == key)
                    })
                });
                if let Some(n) = same {
                    static SAID: std::sync::atomic::AtomicU32 =
                        std::sync::atomic::AtomicU32::new(0);
                    if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
                        crate::vklog!("a pool slot names released texture ser{serial} ({}x{} format {:#x} at {:#x}): the live ser{n} over the same storage is bound (NXGFX_TAKEOVER_FORWARD=0 off)", t.d.width, t.d.height, t.d.format, t.mem as u64);
                    }
                    return n;
                }
            }
        }
        serial
    }
}

/// A six-face copy of a one-layer 2D image that shaders sample as a cube (`Gpu::cube_twin_view`).
struct CubeTwin {
    /// the source's Vulkan image when the twin was made (a new backing makes a new twin)
    src: vk::Image,
    image: vk::Image,
    alloc: Option<Allocation>,
    views: HashMap<(view_t, i32), vk::ImageView>,
    /// the source's (rewrites, gpu_seq) at the last copy
    version: Option<(u32, u64)>,
    copied_sub: Option<u64>,
}

static CUBE_TWINS: Mutex<Option<HashMap<u64, CubeTwin>>> = Mutex::new(None);

/// Whether an image is a square, single-sample, one-layer 2D colour image: the shape a cube twin copies.
pub fn cube_twin_fits(
    ty: vk::ImageType,
    extent: vk::Extent3D,
    layers: u32,
    samples: vk::SampleCountFlags,
    kind: Kind,
) -> bool {
    ty == vk::ImageType::TYPE_2D
        && extent.width == extent.height
        && extent.depth == 1
        && layers == 1
        && samples == vk::SampleCountFlags::TYPE_1
        && !matches!(kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil)
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

/// A view overrides the texture's channel mapping; both select channels of the underlying format.
fn components(t: &tex_t, v: Option<&view_t>, tf: &TexFormat) -> vk::ComponentMapping {
    let s = t.swizzle();
    let mut c = [s & 7, s >> 3 & 7, s >> 6 & 7, s >> 9 & 7];
    if let Some(v) = v.filter(|v| v.bits & VW_SWIZZLE != 0) {
        // NVN view state replaces builder state (nxnative treg_swizzle). Composing two rotations instead reads a
        // different channel: a shadow mask can become an emissive weight in a shader that samples its alpha.
        c = v.swz.map(u16::from);
    }
    if tf.alpha_one {
        for x in c.iter_mut() {
            if *x == 5 {
                *x = 1;
            }
        }
    }
    vk::ComponentMapping {
        r: swz(c[0]),
        g: swz(c[1]),
        b: swz(c[2]),
        a: swz(c[3]),
    }
}

/// `NXGFX_WATCH_ADDR=<hex>`: images made / released over that address are logged (diagnostics).
pub fn watch_log(what: &str, t: &tex_t, gpu: Option<bool>) {
    static W: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let Some(a) = *W.get_or_init(|| {
        std::env::var("NXGFX_WATCH_ADDR")
            .ok()
            .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
    }) else {
        return;
    };
    let lo = t.mem as u64;
    if t.mem.is_null() || a < lo || a >= lo + t.l.size.max(1) {
        return;
    }
    crate::vklog!(
        "watch {:#x}: {what} ser{} {}x{}x{} format {:#x} flags {:#x} target {} levels {} at {lo:#x} (+{:#x}){}",
        a,
        t.serial,
        t.d.width,
        t.d.height,
        t.d.depth,
        t.d.format,
        t.d.flags,
        t.d.target,
        t.d.levels,
        t.l.size,
        gpu.map(|g| format!(", drawn by the GPU: {g}")).unwrap_or_default()
    );
}

/// The texture's life over `NXGFX_WATCH_ADDR` (made, aliased, taken over, parked, reused, handed to an heir, made again
/// with storage use, destroyed, a pass's attachment): one line each with the Vulkan image, for a dead image a pass reads.
pub fn life_log(t: &tex_t, what: impl FnOnce() -> String) {
    static W: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let Some(a) = *W.get_or_init(|| {
        std::env::var("NXGFX_WATCH_ADDR")
            .ok()
            .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
    }) else {
        return;
    };
    let lo = t.mem as u64;
    if t.mem.is_null() || a < lo || a >= lo + t.l.size.max(1) {
        return;
    }
    crate::vklog!(
        "life {a:#x}: ser{} at frame {}: {}",
        t.serial,
        super::present::frames(),
        what()
    );
}

fn img_desc(im: &Image) -> String {
    format!(
        "image {:#x}{}{} {}x{}x{} gpu_owned {}",
        vk::Handle::as_raw(im.image),
        if im.alloc.is_some() {
            " with its memory"
        } else {
            " no memory of its own"
        },
        im.alias_of
            .map(|o| format!(" (alias of ser{o})"))
            .unwrap_or_default(),
        im.extent.width,
        im.extent.height,
        im.layers,
        im.gpu_owned
    )
}

fn shape(t: &tex_t) -> (vk::ImageType, vk::Extent3D, u32) {
    let d = &t.d;
    let w = d.width.max(1);
    let h = d.height.max(1);
    let z = d.depth.max(1);
    match d.target {
        target::T1D => (
            vk::ImageType::TYPE_1D,
            vk::Extent3D {
                width: w,
                height: 1,
                depth: 1,
            },
            1,
        ),
        target::T1D_ARRAY => (
            vk::ImageType::TYPE_1D,
            vk::Extent3D {
                width: w,
                height: 1,
                depth: 1,
            },
            h,
        ),
        target::T3D => (
            vk::ImageType::TYPE_3D,
            vk::Extent3D {
                width: w,
                height: h,
                depth: z,
            },
            1,
        ),
        target::T2D_ARRAY | target::T2D_MS_ARRAY | target::CUBE_ARRAY => (
            vk::ImageType::TYPE_2D,
            vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
            z,
        ),
        target::CUBE => (
            vk::ImageType::TYPE_2D,
            vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
            6,
        ),
        _ => (
            vk::ImageType::TYPE_2D,
            vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
            1,
        ),
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
                crate::vklog!(
                    "image {n} ({} MB made so far): {}x{}x{} format {:#x} target {}, {} MB",
                    b >> 20,
                    t.d.width,
                    t.d.height,
                    t.d.depth,
                    t.d.format,
                    t.d.target,
                    t.l.size >> 20
                );
            }
        }
        let Some(mut tf) = format::tex_format(t.d.format) else {
            crate::log::red_once(&format!(
                "texture format {:#x} has no Vulkan format",
                t.d.format
            ));
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
            let big = extent.width >= 256
                && extent.height >= 256
                && std::env::var("NXGFX_ASTC").as_deref() != Ok("rgba8");
            if big
                && !self
                    .d
                    .format_features(vk::Format::BC7_UNORM_BLOCK)
                    .is_empty()
            {
                tf.vk = if srgb {
                    vk::Format::BC7_SRGB_BLOCK
                } else {
                    vk::Format::BC7_UNORM_BLOCK
                };
                bc7 = true;
            }
        }
        let feat = self.d.format_features(tf.vk);
        if feat.is_empty() {
            crate::log::red_once(&format!(
                "format {:?} (NVN {:#x}) is not supported by the device",
                tf.vk, t.d.format
            ));
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
        let mut flags = vk::ImageCreateFlags::empty();
        if samples == vk::SampleCountFlags::TYPE_1 {
            if feat.contains(vk::FormatFeatureFlags::STORAGE_IMAGE)
                && (t.d.flags as u32 & tex_flags::IMAGE != 0)
            {
                usage |= vk::ImageUsageFlags::STORAGE;
            } else if t.d.flags as u32 & tex_flags::IMAGE == 0 && tf.astc.is_none() && img_wanted(t)
            {
                // used as an image without the flag (`want_image`): storage use, through the UNORM twin when the
                // format itself has none
                if feat.contains(vk::FormatFeatureFlags::STORAGE_IMAGE) {
                    usage |= vk::ImageUsageFlags::STORAGE;
                } else if unorm_twin(tf.vk).is_some_and(|u| {
                    self.d
                        .format_features(u)
                        .contains(vk::FormatFeatureFlags::STORAGE_IMAGE)
                }) {
                    usage |= vk::ImageUsageFlags::STORAGE;
                    flags |= vk::ImageCreateFlags::EXTENDED_USAGE;
                }
            }
        }
        if ty == vk::ImageType::TYPE_3D && usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) {
            flags |= vk::ImageCreateFlags::TYPE_2D_ARRAY_COMPATIBLE;
        }
        let compressed = tf.astc.is_none() && layout::fmt_bpb(t.d.format).1 > 1 || bc7;
        if !compressed
            && tf.kind != Kind::Depth
            && tf.kind != Kind::DepthStencil
            && tf.kind != Kind::Stencil
        {
            flags |= vk::ImageCreateFlags::MUTABLE_FORMAT;
        }
        // any square 2D image of whole cubes: games view 2D arrays as cubes and cubes as cube arrays
        if ty == vk::ImageType::TYPE_2D
            && extent.width == extent.height
            && layers >= 6
            && layers % 6 == 0
            && samples == vk::SampleCountFlags::TYPE_1
        {
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
                    // The delegated sparse creator owns its final create input; no inference here.
                    creation: None,
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
                    view_metadata: HashMap::new(),
                    gpu_owned: true,
                    needs_init: true,
                    needs_upload: false,
                    bc7,
                    decoded: None,
                    sparse: None,
                    page_hash: Vec::new(),
                    rewrites: 0,
                    hashed_early: false,
                    alias_of: None,
                    gpu_seq: 0,
                    scale: 1.0,
                });
            }
        }
        let image = match unsafe { self.dev().create_image(&ci, None) } {
            Ok(i) => i,
            Err(e) => {
                crate::log::red(&format!(
                    "vkCreateImage {:?} {}x{}x{} levels {levels} layers {layers}: {e}",
                    tf.vk, extent.width, extent.height, extent.depth
                ));
                return None;
            }
        };
        let req = unsafe { self.dev().get_image_memory_requirements(image) };
        let a = self
            .alloc
            .lock()
            .unwrap()
            .allocate(&AllocationCreateDesc {
                name: "texture",
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .ok()?;
        unsafe { self.dev().bind_image_memory(image, a.memory(), a.offset()) }.ok()?;
        Some(Image {
            tex: *t,
            image,
            creation: note_creation(image, &ci),
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
            view_metadata: HashMap::new(),
            gpu_owned: false,
            needs_init: true,
            needs_upload: from_storage(t),
            bc7,
            decoded: None,
            sparse: None,
            page_hash: Vec::new(),
            rewrites: 0,
            hashed_early: false,
            scale,
            alias_of: None,
            gpu_seq: 0,
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
        watch_log("image", t, None);
        let how = std::cell::Cell::new("made new");
        // the same texture initialized again over the same memory: its released image, with its content
        let key = tex_identity(t);
        let reused = imgs
            .parked
            .get_mut(&key)
            .and_then(|v| v.pop())
            .map(|(mut im, _)| {
                imgs.parked_n -= 1;
                // pool slots registered with the released object find the image through the new one
                if forward_on() && im.tex.serial != t.serial {
                    if imgs.forward.len() >= 1 << 16 {
                        imgs.forward.clear();
                    }
                    imgs.forward.insert(im.tex.serial, t.serial);
                }
                if im.tex.swz != t.swz || im.tex.bits != t.bits {
                    // views carry the swizzle: made again
                    let views: Vec<vk::ImageView> = im.views.drain().map(|(_, v)| v).collect();
                    im.view_metadata.clear();
                    self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                        for v in views {
                            g.dev().destroy_image_view(v, None);
                        }
                    }));
                }
                im.tex = *t;
                if !im.gpu_owned
                    && im.sparse.is_none()
                    && !im.page_hash.is_empty()
                    && !t.mem.is_null()
                {
                    let lo = t.mem as u64;
                    let mut ph = std::mem::take(&mut im.page_hash);
                    im.needs_upload |= pages_changed(&mut ph, lo, lo + t.l.size, lo, t.l.size);
                    im.page_hash = ph;
                } else if !im.gpu_owned {
                    im.needs_upload = from_storage(t);
                }
                super::perf::inc(super::perf::C::ImagesReused);
                how.set("a parked image reused");
                im
            });
        // a live image of a texture object with the same storage and description, drawn by the GPU: the new
        // object takes it over (Mario Kart 8 Deluxe initializes new objects over its render targets' storage every
        // frame without finalizing the old ones: each was a new image uploaded from storage, 15-50 ms a frame, and
        // the old ones were never freed)
        let reused = reused.or_else(|| {
            let old = imgs
                .by_addr
                .get(&(t.mem as u64))?
                .iter()
                .rev()
                .copied()
                .find(|s| {
                    imgs.by_serial
                        .get(s)
                        .is_some_and(|im| im.gpu_owned && tex_identity(&im.tex) == key)
                })?;
            let mut im = imgs.by_serial.remove(&old)?;
            if let Some(v) = imgs.by_addr.get_mut(&(t.mem as u64)) {
                v.retain(|s| *s != old);
            }
            if forward_on() {
                if imgs.forward.len() >= 1 << 16 {
                    imgs.forward.clear();
                }
                imgs.forward.insert(old, t.serial);
            }
            for a in imgs.by_serial.values_mut() {
                if a.alias_of == Some(old) {
                    a.alias_of = Some(t.serial);
                }
            }
            if im.tex.swz != t.swz || im.tex.bits != t.bits {
                let views: Vec<vk::ImageView> = im.views.drain().map(|(_, v)| v).collect();
                im.view_metadata.clear();
                self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                    for v in views {
                        g.dev().destroy_image_view(v, None);
                    }
                }));
            }
            im.tex = *t;
            super::perf::inc(super::perf::C::ImagesReused);
            how.set("taken over from an older object");
            life_log(t, || format!("takes over the image of ser{old}"));
            Some(im)
        });
        let reused = reused.or_else(|| {
            let a = self.alias_image(imgs, t);
            if a.is_some() {
                how.set("an alias image");
            }
            a
        });
        let Some(im) = reused.or_else(|| self.create_image(t)) else {
            return false;
        };
        life_log(t, || format!("{}: {}", how.get(), img_desc(&im)));
        if !t.mem.is_null() {
            // textures of sparse virtual pools are checked through their pages, not by address (a 5.8 GB terrain
            // array in this map made every CPU write scan every image: Mario Kart World's first frame took 76 s)
            if !crate::nvn::vpool::is_sparse(t.mem as u64) {
                imgs.by_addr.entry(t.mem as u64).or_default().push(t.serial);
                imgs.max_size = imgs.max_size.max(t.l.size);
            }
        }
        imgs.forward.remove(&t.serial);
        imgs.released.remove(&t.serial);
        imgs.by_serial.insert(t.serial, im);
        self.tables.invalidate_images();
        true
    }

    /// A new texture object over the storage of a live image of the same shape (size, levels, layers, samples, bytes
    /// per texel; formats the image can be viewed as): an image that views that one. On the GPU both objects name the
    /// same memory, so what is drawn through one is read through the other; separate images left the second one with
    /// the storage's stale bytes (Beach Bounce's final copy to the window read zeros: lane W2).
    fn alias_image(&self, imgs: &Images, t: &tex_t) -> Option<Image> {
        static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *OFF.get_or_init(|| std::env::var("NXGFX_ALIAS").as_deref() == Ok("0"))
            || t.mem.is_null()
            || crate::nvn::vpool::is_sparse(t.mem as u64)
        {
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
        let (os, o) = imgs
            .by_addr
            .get(&(t.mem as u64))?
            .iter()
            .rev()
            .find_map(|s| {
                let o = imgs.by_serial.get(s)?;
                let (_, oext, _) = shape(&o.tex);
                let same_shape = o.ty == ty
                    && oext == extent
                    && o.levels == levels
                    && o.layers == layers
                    && o.samples == samples
                    && o.sparse.is_none()
                    && !o.bc7;
                let same_fmt = o.format == tf.vk
                    || !depthy(o.tf.kind)
                        && !depthy(tf.kind)
                        && o.tf.astc.is_none()
                        && layout::fmt_bpb(o.tex.d.format) == (bpb, 1, 1)
                        && o.alias_of.is_none()
                        && o.alloc.is_some();
                let storage_ok = t.d.flags as u32 & tex_flags::IMAGE == 0
                    || o.usage.contains(vk::ImageUsageFlags::STORAGE);
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
            creation: o.backing_creation(),
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
            view_metadata: HashMap::new(),
            gpu_owned: o.gpu_owned,
            needs_init: false,
            // the owner's content not uploaded yet: the same storage, uploaded through whichever is used first
            needs_upload: o.needs_upload && !o.gpu_owned,
            bc7: false,
            decoded: None,
            sparse: None,
            page_hash: if t.l.size == o.tex.l.size {
                o.page_hash.clone()
            } else {
                Vec::new()
            },
            rewrites: 0,
            hashed_early: false,
            scale: o.scale,
            alias_of: Some(owner),
            gpu_seq: 0,
        })
    }

    /// A view of an image (created once per view key).
    pub fn image_view(
        &self,
        im: &mut Image,
        v: Option<&view_t>,
        attach: bool,
    ) -> Option<vk::ImageView> {
        self.image_view_kind(im, v, attach, false)
    }

    /// The cube view of a square one-layer 2D texture that a shader samples as a cube (`want`: CUBE or CUBE_ARRAY;
    /// Pokemon Legends Z-A registers its 32x32 BC7 fallback, target 1, into slots its shaders sample as cubes). On
    /// the console the texture descriptor, not the instruction, decides the fetch: a 2D descriptor has one layer,
    /// which every face reads. A Vulkan image of one layer cannot take a cube view, so a cube-compatible twin holds
    /// that layer in all six faces (every level), copied in the submission's upload buffer whenever the texture's
    /// content may have changed (a CPU rewrite, a GPU write, or once per submission for a GPU-written texture).
    pub fn cube_twin_view(
        &self,
        im: &Image,
        v: Option<&view_t>,
        want: i32,
        sub: &mut Sub,
    ) -> Option<vk::ImageView> {
        if !cube_twin_fits(im.ty, im.extent, im.layers, im.samples, im.tf.kind)
            || im.sparse.is_some()
            || !matches!(want, target::CUBE | target::CUBE_ARRAY)
        {
            return None;
        }
        let serial = im.tex.serial;
        let mut all = CUBE_TWINS.lock().unwrap();
        let m = all.get_or_insert_with(HashMap::new);
        if m.get(&serial).is_some_and(|t| t.src != im.image) {
            if let Some(old) = m.remove(&serial) {
                self.drop_cube_twin(old);
            }
        }
        if !m.contains_key(&serial) {
            let twin = self.make_cube_twin(im, sub)?;
            crate::vklog!(
                "texture ser{serial} ({}x{} {:?}, {} levels, target {}) sampled as a cube: a six-face twin of its one layer",
                im.extent.width, im.extent.height, im.format, im.levels, im.tex.d.target
            );
            m.insert(serial, twin);
        }
        let t = m.get_mut(&serial).unwrap();
        let version = (im.rewrites, im.gpu_seq);
        if (t.version != Some(version) || im.gpu_owned) && t.copied_sub != Some(sub.serial) {
            self.copy_cube_twin(im, t.image, sub.pre);
            t.version = Some(version);
            t.copied_sub = Some(sub.serial);
        }
        let key = (v.copied().unwrap_or_default(), want);
        if let Some(&view) = t.views.get(&key) {
            return Some(view);
        }
        let (mut base_level, mut nlevels) = (0, im.levels);
        if let Some(v) = v.filter(|v| v.bits & VW_LEVELS != 0) {
            base_level = (v.base_level as u32).min(im.levels - 1);
            nlevels = (v.nlevels as u32).max(1).min(im.levels - base_level);
        }
        let ci = vk::ImageViewCreateInfo::default()
            .image(t.image)
            .view_type(view_type(want))
            .format(im.format)
            .components(components(&im.tex, v, &im.tf))
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: base_level,
                level_count: nlevels,
                base_array_layer: 0,
                layer_count: 6,
            });
        let view = match unsafe { self.dev().create_image_view(&ci, None) } {
            Ok(v) => v,
            Err(e) => {
                crate::log::red(&format!("vkCreateImageView (cube twin): {e}"));
                return None;
            }
        };
        t.views.insert(key, view);
        Some(view)
    }

    fn make_cube_twin(&self, im: &Image, sub: &mut Sub) -> Option<CubeTwin> {
        let ci = vk::ImageCreateInfo::default()
            .flags(vk::ImageCreateFlags::CUBE_COMPATIBLE)
            .image_type(vk::ImageType::TYPE_2D)
            .format(im.format)
            .extent(im.extent)
            .mip_levels(im.levels)
            .array_layers(6)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = match unsafe { self.dev().create_image(&ci, None) } {
            Ok(i) => i,
            Err(e) => {
                crate::log::red_once(&format!("vkCreateImage (cube twin) {:?}: {e}", im.format));
                return None;
            }
        };
        let req = unsafe { self.dev().get_image_memory_requirements(image) };
        let a = self.alloc.lock().unwrap().allocate(&AllocationCreateDesc {
            name: "cube twin",
            requirements: req,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        });
        let Ok(a) = a else {
            unsafe { self.dev().destroy_image(image, None) };
            return None;
        };
        if unsafe { self.dev().bind_image_memory(image, a.memory(), a.offset()) }.is_err() {
            unsafe { self.dev().destroy_image(image, None) };
            let _ = self.alloc.lock().unwrap().free(a);
            return None;
        }
        // into GENERAL, the layout every texture image is kept in
        let b = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::MEMORY_WRITE | vk::AccessFlags2::MEMORY_READ)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: vk::REMAINING_MIP_LEVELS,
                base_array_layer: 0,
                layer_count: vk::REMAINING_ARRAY_LAYERS,
            })];
        unsafe {
            self.dev().cmd_pipeline_barrier2(
                sub.pre,
                &vk::DependencyInfo::default().image_memory_barriers(&b),
            )
        };
        Some(CubeTwin {
            src: im.image,
            image,
            alloc: Some(a),
            views: HashMap::new(),
            version: None,
            copied_sub: None,
        })
    }

    /// Every level of the source's one layer into each of the twin's six faces.
    fn copy_cube_twin(&self, im: &Image, twin: vk::Image, cb: vk::CommandBuffer) {
        let regions: Vec<vk::ImageCopy> = (0..im.levels)
            .flat_map(|level| {
                let extent = vk::Extent3D {
                    width: (im.extent.width >> level).max(1),
                    height: (im.extent.height >> level).max(1),
                    depth: 1,
                };
                (0..6).map(move |face| vk::ImageCopy {
                    src_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: level,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    src_offset: vk::Offset3D::default(),
                    dst_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: level,
                        base_array_layer: face,
                        layer_count: 1,
                    },
                    dst_offset: vk::Offset3D::default(),
                    extent,
                })
            })
            .collect();
        super::queue::full_barrier(self, cb);
        unsafe {
            self.dev().cmd_copy_image(
                cb,
                im.image,
                vk::ImageLayout::GENERAL,
                twin,
                vk::ImageLayout::GENERAL,
                &regions,
            )
        };
        super::queue::full_barrier(self, cb);
    }

    fn drop_cube_twin(&self, t: CubeTwin) {
        self.defer_image_free(Box::new(move |g: &Gpu| unsafe {
            for v in t.views.values() {
                g.dev().destroy_image_view(*v, None);
            }
            g.dev().destroy_image(t.image, None);
            if let Some(a) = t.alloc {
                let _ = g.alloc.lock().unwrap().free(a);
            }
        }));
    }

    /// The texture goes: its cube twin too.
    fn forget_cube_twin(&self, serial: u64) {
        let twin = CUBE_TWINS
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|m| m.remove(&serial));
        if let Some(t) = twin {
            self.drop_cube_twin(t);
        }
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
                    if im.usage.contains(vk::ImageUsageFlags::STORAGE)
                        || f.vk == fmt
                        || !matches!(f.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil)
                    {
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

    fn image_view_kind(
        &self,
        im: &mut Image,
        v: Option<&view_t>,
        attach: bool,
        storage: bool,
    ) -> Option<vk::ImageView> {
        let k = ViewKey {
            view: v.copied().unwrap_or_default(),
            has: v.is_some(),
            attach,
            storage,
        };
        if let Some(x) = im.views.get(&k) {
            return Some(*x);
        }
        let t = im.tex;
        let mut ty = view_type(t.d.target);
        let mut fmt = im.format;
        let (mut base_level, mut nlevels, mut base_layer, mut nlayers) =
            (0, im.levels, 0, im.layers);
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
            asp = if t.bits & 2 != 0 {
                vk::ImageAspectFlags::STENCIL
            } else {
                vk::ImageAspectFlags::DEPTH
            };
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
        if storage
            && !self
                .d
                .format_features(fmt)
                .contains(vk::FormatFeatureFlags::STORAGE_IMAGE)
        {
            if let Some(u) = unorm_twin(fmt).filter(|&u| {
                self.d
                    .format_features(u)
                    .contains(vk::FormatFeatureFlags::STORAGE_IMAGE)
            }) {
                fmt = u;
            }
        }
        let comps = if attach || storage {
            vk::ComponentMapping::default()
        } else {
            components(&t, v, &im.tf)
        };
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
            crate::log::red_once(&format!(
                "a view of a {:?} image as {fmt:?} has no usage the image allows ({})",
                im.format,
                if attach {
                    "attachment"
                } else if storage {
                    "storage"
                } else {
                    "sampled"
                }
            ));
            return None;
        }
        let usage_ci = vk::ImageViewUsageCreateInfo::default().usage(vusage);
        let mut usage_ci = usage_ci;
        let ci = vk::ImageViewCreateInfo::default()
            .image(im.image)
            .view_type(ty)
            .format(fmt)
            .components(comps)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: asp,
                base_mip_level: base_level,
                level_count: nlevels,
                base_array_layer: base_layer,
                layer_count: nlayers,
            })
            .push_next(&mut usage_ci);
        let view = match unsafe { self.dev().create_image_view(&ci, None) } {
            Ok(v) => v,
            Err(e) => {
                crate::log::red(&format!("vkCreateImageView: {e}"));
                return None;
            }
        };
        im.views.insert(k, view);
        im.view_metadata.insert(k, ResolvedView::from_create_info(view, &ci));
        note_view(view, &ci);
        Some(view)
    }

    /// nvnTexturePoolRegisterTexture / RegisterImage at host ID `id`. A texture registered for sampling whose image does
    /// not exist yet gets it at its first use (`materialize`, mkw-recomp's slots): Mario Kart World registers ~38,800
    /// textures, 7.2 GB of images, most never drawn in a race. `NXGFX_LAZY_IMAGES=0`: every image at registration.
    pub fn reg_tex(&self, id: i32, t: &tex_t, v: Option<view_t>, as_image: bool) {
        super::capture::tex_reg(id, t, v, as_image);
        lazy::forget_id(id);
        if !as_image
            && t.valid()
            && t.d.target != target::BUFFER
            && lazy::on()
            && !self
                .images
                .lock()
                .unwrap()
                .by_serial
                .contains_key(&t.serial)
        {
            self.tables.note_tex(id, t.serial, v);
            lazy::put(id, *t, v);
            return;
        }
        self.reg_tex_now(id, t, v, as_image);
    }

    /// The image of texture ID `id` registered lazily, made now (its first use). Cheap when nothing waits.
    pub fn materialize(&self, id: i32) {
        if let Some((t, v, gone)) = lazy::take(id) {
            self.reg_tex_now(id, &t, v, false);
            if gone {
                self.release_after_use(t.serial);
            }
        }
    }

    /// The image of a texture object finalized before its first use, made now for that use: released (parked, its
    /// slots kept until it is evicted) once the submission being recorded can no longer use it.
    fn release_after_use(&self, serial: u64) {
        self.q.defer_after_use(Box::new(move |g: &Gpu| g.tex_release_now(serial)));
    }

    /// Every lazily registered texture made now (a shader indexes the texture pool itself: any ID may be read).
    pub fn materialize_all(&self) {
        let all = lazy::take_all();
        if all.is_empty() {
            return;
        }
        let n = all.len();
        for (id, (t, v, gone)) in all {
            self.reg_tex_now(id, &t, v, false);
            if gone {
                self.release_after_use(t.serial);
            }
        }
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| {
            crate::vklog!(
                "lazy images: a shader indexing the texture pool made the {n} waiting ones"
            )
        });
    }

    /// Pool-indexed shaders can reach any registered image, including ones made after image_begin_submit.
    pub fn prepare_pool_images(&self, sub: &mut Sub) {
        self.materialize_all();
        let due = self.tables.take_image_pending();
        let mut imgs = self.images.lock().unwrap();
        super::perf::inc(super::perf::C::PoolPrepareScans);
        super::perf::add(super::perf::C::PoolPrepareImages, due.len() as u64);
        for serial in due {
            let serial = imgs.live(serial);
            self.prepare_image(&mut imgs, serial, sub);
        }
    }

    fn reg_tex_now(&self, id: i32, t: &tex_t, v: Option<view_t>, as_image: bool) {
        if !t.valid() {
            crate::log::red_once("RegisterTexture of an uninitialized texture object");
            return;
        }
        if id < 0 || id as u32 >= self.layout.tex_cap {
            crate::log::red_once(&format!(
                "texture ID {id} past the pool set (NXGFX_TEX_CAP)"
            ));
            return;
        }
        if t.d.target == target::BUFFER {
            // a buffer texture: no image; its texel buffer views are made over its pool's memory
            self.tables.note_buf_tex(id, *t);
            return;
        }
        if as_image {
            want_image(t);
        }
        let mut imgs = self.images.lock().unwrap();
        if !self.ensure_image(&mut imgs, t) {
            return;
        }
        let im = imgs.by_serial.get_mut(&t.serial).unwrap();
        if im.needs_init || im.needs_upload {
            self.tables.note_image_pending(t.serial);
        }
        let storage = im.usage.contains(vk::ImageUsageFlags::STORAGE)
            && (as_image || t.d.flags as u32 & tex_flags::IMAGE != 0 || img_wanted(t));
        // Registration permits stores; it is not a GPU write. Initial storage content must still be uploaded.
        let Some(view) = self.image_view(im, v.as_ref(), false) else {
            return;
        };
        let sview = if storage {
            self.storage_view(im, v.as_ref())
        } else {
            None
        };
        drop(imgs);
        self.tables.note_tex(id, t.serial, v);
        self.tables.write_image(&self.d, id, view, sview);
    }

    /// nvnTextureFinalize: the image goes once the GPU no longer uses it.
    pub fn tex_release(&self, serial: u64) {
        // in queue order: the work the game queued before finalizing the texture still binds it (asynchronous replay
        // runs behind the game; released at once, Mario Kart 8 Deluxe's scene target ser6754 went from the table while
        // that frame's tone mapping, still queued, read it through its pool slots: the black stretches).
        // NXGFX_RELEASE_ORDER=0: at once, as before.
        static ORDER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *ORDER.get_or_init(|| std::env::var("NXGFX_RELEASE_ORDER").as_deref() != Ok("0"))
            && super::queue::aq_busy()
        {
            super::queue::after_queued(Box::new(move |g: &'static Gpu| g.tex_release_now(serial)));
            return;
        }
        self.tex_release_now(serial);
    }

    fn tex_release_now(&self, serial: u64) {
        self.tables.invalidate_images();
        super::capture::tex_release(serial);
        lazy::finalized(serial);
        self.forget_cube_twin(serial);
        let mut imgs = self.images.lock().unwrap();
        let Some(mut im) = imgs.by_serial.remove(&serial) else {
            return;
        };
        watch_log("released", &im.tex, Some(im.gpu_owned));
        if imgs.released.len() >= 1 << 16 {
            imgs.released.clear();
        }
        imgs.released.insert(serial, im.tex);
        if let Some(v) = imgs.by_addr.get_mut(&(im.tex.mem as u64)) {
            v.retain(|s| *s != serial);
        }
        // an image shared with other texture objects: an alias only drops its views; an owner hands the image and its
        // memory to the first of its aliases
        let aliases: Vec<u64> = imgs
            .by_serial
            .iter()
            .filter(|(_, a)| a.alias_of == Some(serial))
            .map(|(k, _)| *k)
            .collect();
        life_log(&im.tex, || {
            format!("released ({} aliases): {}", aliases.len(), img_desc(&im))
        });
        if im.alias_of.is_some() || !aliases.is_empty() {
            if let Some((&heir, rest)) = aliases.split_first() {
                life_log(&im.tex, || format!("its image and memory go to ser{heir}"));
                let a = imgs.by_serial.get_mut(&heir).unwrap();
                a.promote_from(&mut im);
                for r in rest {
                    imgs.by_serial.get_mut(r).unwrap().alias_of = Some(heir);
                }
            }
            let views: Vec<vk::ImageView> = im.views.drain().map(|(_, v)| v).collect();
            im.view_metadata.clear();
            drop(imgs);
            self.scrub_slots(serial, im.tex.d.target);
            self.defer_image_free(Box::new(move |g: &Gpu| unsafe {
                for v in views {
                    g.dev().destroy_image_view(v, None);
                }
            }));
            return;
        }
        // kept a few frames for a texture initialized again the same way (ensure_image)
        let key = tex_identity(&im.tex);
        let frame = super::present::frames();
        life_log(&im.tex, || "parked".to_string());
        imgs.parked.entry(key).or_default().push((im, frame));
        imgs.parked_n += 1;
        let old = self.evict_parked(&mut imgs, frame);
        drop(imgs);
        for im in old {
            self.scrub_slots(im.tex.serial, im.tex.d.target);
            self.destroy_released(im);
        }
    }

    /// VRAM budget (Turbo `vk_budget`, its default; `NXGFX_VRAM_EVICT=0` off): once a frame, the device-local heaps'
    /// use against their budget (VK_EXT_memory_budget). Over 90% every parked image is freed (the render targets and
    /// textures kept for a re-initialization the same way, which the game has let go), then the pipeline cache's
    /// retired pipelines are left as they are. Returns the bytes freed.
    pub fn vram_budget(&self) -> u64 {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !self.d.caps.mem_budget
            || !*ON.get_or_init(|| std::env::var("NXGFX_VRAM_EVICT").as_deref() != Ok("0"))
        {
            return 0;
        }
        let mut b = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
        let mut p2 = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut b);
        unsafe {
            self.d
                .instance
                .get_physical_device_memory_properties2(self.d.pdev, &mut p2)
        };
        let mp = p2.memory_properties;
        let (mut used, mut budget) = (0u64, 0u64);
        for h in 0..mp.memory_heap_count as usize {
            if mp.memory_heaps[h]
                .flags
                .contains(vk::MemoryHeapFlags::DEVICE_LOCAL)
            {
                used += b.heap_usage[h];
                budget += b.heap_budget[h];
            }
        }
        let pct = std::env::var("NXGFX_VRAM_EVICT_PCT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(90);
        if budget == 0 || used * 100 < budget * pct {
            return 0;
        }
        let mut imgs = self.images.lock().unwrap();
        let mut gone = Vec::new();
        for (_, v) in imgs.parked.drain() {
            gone.extend(v.into_iter().map(|x| x.0));
        }
        imgs.parked_n = 0;
        drop(imgs);
        let n = gone.len();
        let bytes: u64 = gone
            .iter()
            .filter_map(|im| im.alloc.as_ref().map(|a| a.size()))
            .sum();
        for im in gone {
            self.scrub_slots(im.tex.serial, im.tex.d.target);
            self.destroy_released(im);
        }
        static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16 {
            crate::vklog!("VRAM: {} of {} MB in use (over {pct}% of the budget): {n} parked images freed ({} MB)", used >> 20, budget >> 20, bytes >> 20);
        }
        bytes
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
        let serials: Vec<u64> = imgs
            .by_serial
            .iter()
            .filter(|(_, im)| {
                im.scale != s
                    && super::scale::candidate(&im.tex)
                    && im.sparse.is_none()
                    && im.alias_of.is_none()
            })
            .map(|(k, _)| *k)
            .collect();
        let mut done = std::collections::HashSet::new();
        for serial in serials {
            let im = imgs.by_serial.get_mut(&serial).unwrap();
            let (_, native, _) = shape(&im.tex);
            let ext = vk::Extent3D {
                width: super::scale::dim(native.width, s),
                height: super::scale::dim(native.height, s),
                depth: native.depth,
            };
            let mut flags = vk::ImageCreateFlags::empty();
            if !matches!(im.tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                flags |= vk::ImageCreateFlags::MUTABLE_FORMAT;
            }
            if ext.width == ext.height
                && im.layers >= 6
                && im.layers % 6 == 0
                && im.samples == vk::SampleCountFlags::TYPE_1
            {
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
            let Ok(image) = (unsafe { self.dev().create_image(&ci, None) }) else {
                continue;
            };
            let req = unsafe { self.dev().get_image_memory_requirements(image) };
            let Ok(a) = self.alloc.lock().unwrap().allocate(&AllocationCreateDesc {
                name: "texture",
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            }) else {
                unsafe { self.dev().destroy_image(image, None) };
                continue;
            };
            if unsafe { self.dev().bind_image_memory(image, a.memory(), a.offset()) }.is_err() {
                continue;
            }
            let asp = aspect(&im.tf);
            let range = vk::ImageSubresourceRange {
                aspect_mask: asp,
                base_mip_level: 0,
                level_count: im.levels,
                base_array_layer: 0,
                layer_count: im.layers,
            };
            let b = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(image)
                .subresource_range(range)];
            unsafe {
                self.dev().cmd_pipeline_barrier2(
                    sub.pre,
                    &vk::DependencyInfo::default().image_memory_barriers(&b),
                )
            };
            if im.samples == vk::SampleCountFlags::TYPE_1 && !im.needs_init {
                // the content at the new size (blits cannot read multisampled images: those start cleared)
                let flt = super::scale::filter(self, im.format, im.tf.kind);
                for lv in 0..im.levels {
                    let sl = vk::ImageSubresourceLayers {
                        aspect_mask: asp,
                        mip_level: lv,
                        base_array_layer: 0,
                        layer_count: im.layers,
                    };
                    let (ow, oh) = (
                        (im.extent.width >> lv).max(1) as i32,
                        (im.extent.height >> lv).max(1) as i32,
                    );
                    let (nw, nh) = (
                        (ext.width >> lv).max(1) as i32,
                        (ext.height >> lv).max(1) as i32,
                    );
                    let r = vk::ImageBlit {
                        src_subresource: sl,
                        src_offsets: [vk::Offset3D::default(), vk::Offset3D { x: ow, y: oh, z: 1 }],
                        dst_subresource: sl,
                        dst_offsets: [vk::Offset3D::default(), vk::Offset3D { x: nw, y: nh, z: 1 }],
                    };
                    unsafe {
                        self.dev().cmd_blit_image(
                            sub.pre,
                            im.image,
                            vk::ImageLayout::GENERAL,
                            image,
                            vk::ImageLayout::GENERAL,
                            &[r],
                            flt,
                        )
                    };
                }
            } else if !im.needs_init {
                if matches!(im.tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                    let v = vk::ClearDepthStencilValue {
                        depth: 0.0,
                        stencil: 0,
                    };
                    unsafe {
                        self.dev().cmd_clear_depth_stencil_image(
                            sub.pre,
                            image,
                            vk::ImageLayout::GENERAL,
                            &v,
                            &[range],
                        )
                    };
                } else {
                    let v = vk::ClearColorValue { uint32: [0; 4] };
                    unsafe {
                        self.dev().cmd_clear_color_image(
                            sub.pre,
                            image,
                            vk::ImageLayout::GENERAL,
                            &v,
                            &[range],
                        )
                    };
                }
            }
            let (old_image, old_creation) = im.replace_backing(image, note_creation(image, &ci));
            let old = Image {
                tex: im.tex,
                image: old_image,
                creation: old_creation,
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
                view_metadata: HashMap::new(),
                gpu_owned: true,
                needs_init: false,
                needs_upload: false,
                bc7: im.bc7,
                decoded: None,
                sparse: None,
                page_hash: Vec::new(),
                rewrites: 0,
                hashed_early: false,
                scale: im.scale,
                alias_of: None,
                gpu_seq: 0,
            };
            im.extent = ext;
            im.scale = s;
            gone.push(old);
            done.insert(serial);
        }
        super::queue::full_barrier(self, sub.pre);
        // aliases of the images made again view the new ones
        let followers: Vec<u64> = imgs
            .by_serial
            .iter()
            .filter(|(_, a)| a.alias_of.is_some_and(|o| done.contains(&o)))
            .map(|(k, _)| *k)
            .collect();
        for f in followers {
            let o = imgs.by_serial[&imgs.by_serial[&f].alias_of.unwrap()].image;
            let (ext, sc, creation) = {
                let oi = &imgs.by_serial[&imgs.by_serial[&f].alias_of.unwrap()];
                (oi.extent, oi.scale, oi.backing_creation())
            };
            let a = imgs.by_serial.get_mut(&f).unwrap();
            let views: Vec<vk::ImageView> = a.views.drain().map(|(_, v)| v).collect();
            a.view_metadata.clear();
            self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                for v in views {
                    g.dev().destroy_image_view(v, None);
                }
            }));
            a.replace_backing(o, creation);
            a.extent = ext;
            a.scale = sc;
            done.insert(f);
        }
        // the texture pool's descriptors of the images made again
        for (id, serial, v) in self.tables.texture_slots() {
            let serial = imgs.live(serial);
            if !done.contains(&serial) {
                continue;
            }
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

    /// The images over `addrs` (storage of textures used as images without the IMAGE flag) that lack storage use made
    /// again with it, their content copied at `cb`'s current position, the old ones released and the
    /// texture pool's descriptors of every texture over them rewritten. Returns how many were made again.
    /// Mid-replay callers must end rendering first and pass sub.main; submission setup passes sub.pre.
    pub fn upgrade_storage(
        &self,
        imgs: &mut Images,
        addrs: &[u64],
        sub: &mut Sub,
        cb: vk::CommandBuffer,
    ) -> usize {
        let owners: Vec<u64> = imgs
            .by_serial
            .iter()
            .filter(|(_, im)| {
                addrs.contains(&(im.tex.mem as u64))
                    && im.alias_of.is_none()
                    && im.sparse.is_none()
                    && im.alloc.is_some()
                    && !im.usage.contains(vk::ImageUsageFlags::STORAGE)
            })
            .map(|(k, _)| *k)
            .collect();
        let mut done = std::collections::HashSet::new();
        let mut gone = Vec::new();
        for serial in owners {
            let tex = imgs.by_serial[&serial].tex;
            let Some(new) = self.create_image(&tex) else {
                continue;
            };
            let im = &imgs.by_serial[&serial];
            if !new.usage.contains(vk::ImageUsageFlags::STORAGE)
                || new.extent != im.extent
                || new.format != im.format
                || new.sparse.is_some()
            {
                self.destroy_released(new);
                continue;
            }
            // Prepare the old backing before moving it. First content may live in pre, but earlier replay writes
            // live in main: the copy itself must stay at the caller's actual position in that command stream.
            self.prepare_image(imgs, serial, sub);
            let im = imgs.by_serial.get_mut(&serial).unwrap();
            super::queue::full_barrier(self, cb);
            let asp = aspect(&im.tf);
            let range = vk::ImageSubresourceRange {
                aspect_mask: asp,
                base_mip_level: 0,
                level_count: im.levels,
                base_array_layer: 0,
                layer_count: im.layers,
            };
            let b = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(new.image)
                .subresource_range(range)];
            unsafe {
                self.dev().cmd_pipeline_barrier2(
                    cb,
                    &vk::DependencyInfo::default().image_memory_barriers(&b),
                )
            };
            if !im.needs_init {
                let regions: Vec<vk::ImageCopy> = (0..im.levels)
                    .map(|lv| {
                        let sl = vk::ImageSubresourceLayers {
                            aspect_mask: asp,
                            mip_level: lv,
                            base_array_layer: 0,
                            layer_count: if im.ty == vk::ImageType::TYPE_3D {
                                1
                            } else {
                                im.layers
                            },
                        };
                        vk::ImageCopy {
                            src_subresource: sl,
                            src_offset: vk::Offset3D::default(),
                            dst_subresource: sl,
                            dst_offset: vk::Offset3D::default(),
                            extent: vk::Extent3D {
                                width: (im.extent.width >> lv).max(1),
                                height: (im.extent.height >> lv).max(1),
                                depth: (im.extent.depth >> lv).max(1),
                            },
                        }
                    })
                    .collect();
                unsafe {
                    self.dev().cmd_copy_image(
                        cb,
                        im.image,
                        vk::ImageLayout::GENERAL,
                        new.image,
                        vk::ImageLayout::GENERAL,
                        &regions,
                    )
                };
            }
            let mut new = new;
            let (old_image, old_creation) = im.replace_backing(new.image, new.backing_creation());
            let old = Image {
                tex: im.tex,
                image: old_image,
                creation: old_creation,
                alloc: std::mem::replace(&mut im.alloc, new.alloc.take()),
                format: im.format,
                tf: im.tf,
                ty: im.ty,
                extent: im.extent,
                levels: im.levels,
                layers: im.layers,
                samples: im.samples,
                usage: std::mem::replace(&mut im.usage, new.usage),
                views: std::mem::take(&mut im.views),
                view_metadata: HashMap::new(),
                gpu_owned: true,
                needs_init: false,
                needs_upload: false,
                bc7: im.bc7,
                decoded: None,
                sparse: None,
                page_hash: Vec::new(),
                rewrites: 0,
                hashed_early: false,
                scale: im.scale,
                alias_of: None,
                gpu_seq: 0,
            };
            life_log(&im.tex, || {
                format!("made again with storage use: {}", img_desc(im))
            });
            gone.push(old);
            im.needs_init = false;
            im.needs_upload = false;
            done.insert(serial);
        }
        if done.is_empty() {
            return 0;
        }
        super::queue::full_barrier(self, cb);
        self.tables.invalidate_images();
        // aliases view the new images
        let followers: Vec<u64> = imgs
            .by_serial
            .iter()
            .filter(|(_, a)| a.alias_of.is_some_and(|o| done.contains(&o)))
            .map(|(k, _)| *k)
            .collect();
        for f in followers {
            let (o, usage, creation) = {
                let oi = &imgs.by_serial[&imgs.by_serial[&f].alias_of.unwrap()];
                (oi.image, oi.usage, oi.backing_creation())
            };
            let a = imgs.by_serial.get_mut(&f).unwrap();
            let views: Vec<vk::ImageView> = a.views.drain().map(|(_, v)| v).collect();
            a.view_metadata.clear();
            self.q.defer_after_use(Box::new(move |g: &Gpu| unsafe {
                for v in views {
                    g.dev().destroy_image_view(v, None);
                }
            }));
            a.replace_backing(o, creation);
            a.usage = usage;
            a.needs_init = false;
            a.needs_upload = false;
            done.insert(f);
        }
        for (id, serial, v) in self.tables.texture_slots() {
            let serial = imgs.live(serial);
            if !done.contains(&serial) {
                continue;
            }
            if let Some(im) = imgs.by_serial.get_mut(&serial) {
                if let Some(view) = self.image_view(im, v.as_ref(), false) {
                    let sview = self.storage_view(im, v.as_ref());
                    self.tables.write_image(&self.d, id, view, sview);
                }
            }
        }
        sub.memo_views.clear();
        let n = done.len();
        crate::vklog!("{n} image(s) made again with storage use (textures used as images without the IMAGE flag)");
        for im in gone {
            self.destroy_released(im);
        }
        n
    }

    /// An image's (or its views') destruction after the GPU is past its last use; `NXGFX_DESTROY_DELAY=<frames>`
    /// (diagnosis, lane K2) holds it that many presents longer first (MKW's mesh-draw faults: a freed image still read?).
    fn defer_image_free(&self, f: Box<dyn FnOnce(&Gpu) + Send>) {
        static DELAY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        static HELD: std::sync::Mutex<Vec<(u64, Box<dyn FnOnce(&Gpu) + Send>)>> =
            std::sync::Mutex::new(Vec::new());
        let d = *DELAY.get_or_init(|| {
            std::env::var("NXGFX_DESTROY_DELAY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        });
        if d == 0 {
            self.q.defer_after_use(f);
            return;
        }
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| crate::vklog!("NXGFX_DESTROY_DELAY: released images are destroyed {d} presents later than their last use"));
        let now = super::present::frames();
        let due: Vec<Box<dyn FnOnce(&Gpu) + Send>> = {
            let mut h = HELD.lock().unwrap();
            h.push((now + d, f));
            let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut *h)
                .into_iter()
                .partition(|(at, _)| *at <= now);
            *h = keep;
            due.into_iter().map(|(_, f)| f).collect()
        };
        for f in due {
            self.q.defer_after_use(f);
        }
    }

    /// The texture pool's slots that name `serial` get the zero stand-in, before its views go: a slot keeps its
    /// descriptor after the game finalizes the texture (the console's descriptor still points at memory that is
    /// there), and a shader that samples it after the image went read freed memory and lost the device (lane K2:
    /// MKW's device losses at addresses no live allocation held). `NXGFX_SCRUB_SLOTS=0` off.
    fn scrub_slots(&self, serial: u64, tex_target: i32) {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| std::env::var("NXGFX_SCRUB_SLOTS").as_deref() != Ok("0")) {
            return;
        }
        let ids = self.tables.ids_of(&std::iter::once(serial).collect());
        if ids.is_empty() {
            return;
        }
        let mut n = 0;
        for (id, _, v) in ids {
            let target = match v {
                Some(v) if v.bits & VW_TARGET != 0 => v.target as i32,
                _ => tex_target,
            };
            self.tables.mark_placeholder(id);
            let Some(view) = super::exec::dummy_view_ready(self, target, false) else {
                continue;
            };
            let sview = super::exec::dummy_view_ready(self, target, true);
            self.tables.write_image(&self.d, id, view, sview);
            n += 1;
        }
        static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
            crate::vklog!("texture pool: {n} slot(s) of an image that goes now name the zero stand-in (NXGFX_SCRUB_SLOTS=0 off)");
        }
    }

    fn destroy_released(&self, im: Image) {
        life_log(&im.tex, || {
            format!("destroyed (after its last use): {}", img_desc(&im))
        });
        super::lost::note_gone(|| {
            format!(
                "image {:#x} of texture ser{} at {:#x} ({}x{}, {} views, {})",
                vk::Handle::as_raw(im.image),
                im.tex.serial,
                im.tex.mem as u64,
                im.extent.width,
                im.extent.height,
                im.views.len(),
                if im.alloc.is_some() {
                    "with its memory"
                } else {
                    "no memory of its own"
                }
            )
        });
        {
            self.defer_image_free(Box::new(move |g: &Gpu| unsafe {
                for v in im.views.values() {
                    g.dev().destroy_image_view(*v, None);
                }
                super::sparse::destroy(im.image);
                g.dev().destroy_image(im.image, None);
                if let Some(a) = im.alloc {
                    let _ = g.alloc.lock().unwrap().free(a);
                }
            }));
        }
    }

    /// Small textures the GPU wrote in this submission, copied out at its end and written into their storage at its
    /// completion, so a later CPU write to part of one merges with what the GPU drew (the texture is uploaded from its
    /// storage again, which now holds both). Without it a texture the GPU wrote kept every CPU write under half of it
    /// out for good (MKW's CPU-updated RGBA16F tables, lane K2); mkw-recomp writes GPU-written textures back at the
    /// fence (`vk_tex_alias.c` `gpu_written_locked`, `wb_at_fence`). Only single-level, single-layer, uncompressed 2D
    /// textures of at most 512 KB whose image is the storage's size; a CPU write to the storage in the meantime keeps
    /// the old behaviour for that texture (the write-back is dropped). Byte-copy formats only (the image's texels
    /// are the storage's bytes: true for every format let through here). `NXGFX_TEX_WB=1` on (off by default: `K2-mkw16`'s race washed out with it).
    pub fn tex_writeback(&self, sub: &mut Sub, serials: &[u64]) {
        let mut seen = std::collections::HashSet::new();
        let imgs = self.images.lock().unwrap();
        let mut jobs = Vec::new();
        for &s in serials {
            if !seen.insert(s) || jobs.len() >= 64 {
                continue;
            }
            let Some(im) = imgs.by_serial.get(&s) else {
                continue;
            };
            let t = im.tex;
            let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
            let depthy = matches!(im.tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil);
            if t.mem.is_null()
                || t.l.size == 0
                || t.l.size > 512 << 10
                || bpb == 0
                || bw != 1
                || bh != 1
                || depthy
                || im.tf.astc.is_some()
                || im.bc7
                || im.tf.expand3 != 0
            {
                continue;
            }
            if im.levels != 1
                || im.layers != 1
                || im.ty != vk::ImageType::TYPE_2D
                || im.samples != vk::SampleCountFlags::TYPE_1
                || im.sparse.is_some()
                || im.scale != 1.0
            {
                continue;
            }
            if im.extent.width != t.d.width as u32
                || im.extent.height != t.d.height as u32
                || crate::nvn::vpool::is_sparse(t.mem as u64)
            {
                continue;
            }
            jobs.push((s, im.image, im.gpu_seq, t, bpb));
        }
        drop(imgs);
        if jobs.is_empty() {
            return;
        }
        super::queue::full_barrier(self, sub.main);
        for (serial, image, seq, t, bpb) in jobs {
            let (w, h) = (t.d.width as u32, t.d.height as u32);
            let n = w as u64 * h as u64 * bpb as u64;
            let Ok(b) = self.buffer(
                n,
                vk::BufferUsageFlags::TRANSFER_DST,
                gpu_allocator::MemoryLocation::GpuToCpu,
                "texture write-back",
            ) else {
                continue;
            };
            let aspect = vk::ImageAspectFlags::COLOR;
            let r = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: aspect,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                image_offset: vk::Offset3D::default(),
                image_extent: vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                },
            };
            unsafe {
                self.dev().cmd_copy_image_to_buffer(
                    sub.main,
                    image,
                    vk::ImageLayout::GENERAL,
                    b.buf,
                    &[r],
                )
            };
            struct Job(super::Buf, tex_t, u64, u64, vk::Image, u32);
            unsafe impl Send for Job {}
            let job = Job(b, t, serial, seq, image, bpb);
            sub.post
                .push(super::queue::Post::Callback(Box::new(move |g: &Gpu| {
                    // the bytes kept for the next submission's start, which lands them with the image table it holds
                    // anyway (the completion thread found the table busy nearly always: 8,000 of 8,004 dropped)
                    let Job(b, t, serial, seq, image, bpb) = job;
                    let n = t.d.width as usize * t.d.height as usize * bpb as usize;
                    let data =
                        unsafe { core::slice::from_raw_parts(b.ptr as *const u8, n) }.to_vec();
                    g.free_buffer(b);
                    let mut q = WB_LANDING.lock().unwrap();
                    if q.len() < 256 {
                        q.push(WbLanding {
                            t,
                            serial,
                            seq,
                            image,
                            bpb,
                            data,
                        });
                    }
                })));
        }
    }

    /// At completion: the copied texture into its storage, if it is still the texture the GPU wrote last and the CPU
    /// has not written its storage since (then its storage is the texture's content again: not GPU-owned).
    fn tex_writeback_land(&self, imgs: &mut Images, w: &WbLanding) {
        let (t, serial, seq, image, bpb, data) =
            (&w.t, w.serial, w.seq, w.image, w.bpb, w.data.as_ptr());
        let Some(im) = imgs.by_serial.get_mut(&serial) else {
            return;
        };
        if im.image != image || im.gpu_seq != seq || im.tex.mem != t.mem {
            return;
        }
        let (lo, hi) = (
            t.mem as u64 & !0xfff,
            (t.mem as u64 + t.l.size + 0xfff) & !0xfff,
        );
        // CPU writes since (or pages held on the device): the old behaviour for this one
        if super::mem::cpu_written(lo, hi - lo)
            || super::lazy::outside(t.mem as u64, t.l.size).is_some()
            || !super::mem::host_writable(t.mem as u64, t.l.size)
        {
            return;
        }
        let (w, h) = (t.d.width as u32, t.d.height as u32);
        let r = NVNcopyRegion {
            xoffset: 0,
            yoffset: 0,
            zoffset: 0,
            width: w as i32,
            height: h as i32,
            depth: 1,
        };
        let row = w as usize * bpb as usize;
        if write_storage(t, 0, 0, &r, data, row, row * h as usize).is_none() {
            return;
        }
        super::mem::reset_watch(lo, hi - lo);
        im.gpu_owned = false;
        im.needs_upload = false;
        // the page hashes named the old storage bytes: a later CPU write uploads the whole texture
        im.page_hash.clear();
        static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
            crate::vklog!("texture write-back: ser{serial} {w}x{h} fmt {:#x} back in its storage at {:#x} (NXGFX_TEX_WB=1)", t.d.format, t.mem as u64);
        }
    }

    /// The GPU writes a texture: its storage no longer holds its content.
    pub fn tex_gpu_written(&self, imgs: &mut Images, serial: u64) {
        let Some(im) = imgs.by_serial.get_mut(&serial) else {
            return;
        };
        im.gpu_owned = true;
        im.needs_upload = false;
        im.gpu_seq += 1;
        // the texture objects sharing its image
        let (image, at) = (im.image, im.tex.mem as u64);
        let Some(group) = imgs.by_addr.get(&at) else {
            return;
        };
        if group.len() < 2 {
            return;
        }
        for s in group.clone() {
            if let Some(o) = imgs.by_serial.get_mut(&s) {
                if o.image == image {
                    o.gpu_owned = true;
                    o.needs_upload = false;
                    if s != serial {
                        o.gpu_seq += 1;
                    }
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
            crate::log::red_once(&format!(
                "buffer texture format {:#x} has no texel buffer format",
                t.d.format
            ));
            return None;
        };
        let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
        if bw != 1 || bh != 1 || bpb == 0 || tf.expand3 != 0 {
            crate::log::red_once(&format!(
                "buffer texture format {:?} cannot be a texel buffer",
                tf.vk
            ));
            return None;
        }
        let need = if storage {
            vk::FormatFeatureFlags::STORAGE_TEXEL_BUFFER
        } else {
            vk::FormatFeatureFlags::UNIFORM_TEXEL_BUFFER
        };
        let ff = unsafe {
            self.d
                .instance
                .get_physical_device_format_properties(self.d.pdev, tf.vk)
        }
        .buffer_features;
        if !ff.contains(need) {
            if !storage {
                crate::log::red_once(&format!(
                    "buffer texture format {:?} is not a texel buffer format on this device",
                    tf.vk
                ));
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
            crate::log::red_once(&format!(
                "buffer texture at an offset Vulkan cannot view ({off:#x})"
            ));
            return None;
        }
        let ci = vk::BufferViewCreateInfo::default()
            .buffer(buf)
            .format(tf.vk)
            .offset(off)
            .range(n);
        let v = unsafe { self.dev().create_buffer_view(&ci, None) }.ok()?;
        sub.buffer_views.push(v);
        Some(v)
    }

    /// Descriptor buffers: a buffer texture's texel buffer as (device address, bytes, format), the checks of
    /// `texel_view`.
    pub fn texel_addr(
        &self,
        sub: &mut Sub,
        t: &tex_t,
        storage: bool,
    ) -> Option<vk::DescriptorAddressInfoEXT<'static>> {
        let tf = format::tex_format(t.d.format)?;
        let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
        if bw != 1 || bh != 1 || bpb == 0 || tf.expand3 != 0 {
            return None;
        }
        let need = if storage {
            vk::FormatFeatureFlags::STORAGE_TEXEL_BUFFER
        } else {
            vk::FormatFeatureFlags::UNIFORM_TEXEL_BUFFER
        };
        let ff = unsafe {
            self.d
                .instance
                .get_physical_device_format_properties(self.d.pdev, tf.vk)
        }
        .buffer_features;
        if !ff.contains(need) {
            return None;
        }
        let n = (t.d.width as u64).min(self.d.props.limits.max_texel_buffer_elements as u64)
            * bpb as u64;
        if n == 0 || t.mem.is_null() {
            return None;
        }
        let (buf, off, _) = super::mem::resolve(self, sub, t.mem as u64, n)?;
        if off % self.d.props.limits.min_texel_buffer_offset_alignment != 0 {
            crate::log::red_once(&format!(
                "buffer texture at an offset Vulkan cannot view ({off:#x})"
            ));
            return None;
        }
        let base = super::mem::buffer_address(buf)?;
        Some(
            vk::DescriptorAddressInfoEXT::default()
                .address(base + off)
                .range(n)
                .format(tf.vk),
        )
    }

    /// Texel buffer descriptors (uniform, storage) over 64 KB of zeros: a buffer texture whose memory is gone.
    fn texel_dummy(
        &self,
    ) -> Option<(
        vk::DescriptorAddressInfoEXT<'static>,
        vk::DescriptorAddressInfoEXT<'static>,
    )> {
        static B: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
        let a = (*B.get_or_init(|| {
            let usage = vk::BufferUsageFlags::UNIFORM_TEXEL_BUFFER
                | vk::BufferUsageFlags::STORAGE_TEXEL_BUFFER
                | vk::BufferUsageFlags::TRANSFER_DST;
            let b = self
                .buffer(
                    65536,
                    usage,
                    gpu_allocator::MemoryLocation::CpuToGpu,
                    "texel dummy",
                )
                .ok()?;
            if !b.ptr.is_null() {
                unsafe { core::ptr::write_bytes(b.ptr, 0, 65536) };
            }
            let a = b.addr;
            core::mem::forget(b);
            (a != 0).then_some(a)
        }))?;
        let ai = vk::DescriptorAddressInfoEXT::default()
            .address(a)
            .range(65536)
            .format(vk::Format::R32_UINT);
        Some((ai, ai))
    }

    fn texel_view_persistent(
        &self,
        sub: &mut Sub,
        t: &tex_t,
        storage: bool,
    ) -> Option<vk::BufferView> {
        let v = self.texel_view(sub, t, storage)?;
        // the pool set holds it past this submission: it is taken out of the submission's list and kept
        sub.buffer_views.pop();
        Some(v)
    }

    pub fn image_begin_submit(&self, sub: &mut Sub, dirty: &[(u64, u64)]) {
        // This pass prepares every existing pending image. Later registrations enqueue only their own serials
        // for pool first-use preparation; pool draws never need to rescan the full image registry.
        self.tables.take_image_pending();
        // buffer textures registered since the last submission: their pool-set views
        for id in self.tables.take_buf_pending() {
            let Some(t) = self.tables.buf_tex_at(id) else {
                continue;
            };
            if self.d.caps.descbuf {
                match self.texel_addr(sub, &t, false) {
                    Some(u) => {
                        let s = self.texel_addr(sub, &t, true);
                        self.tables
                            .write_texel_addrs(&self.d, &self.layout, id, u, s);
                    }
                    // no memory pool holds it (now): a buffer of zeros instead of the address of a freed buffer
                    None => {
                        if let Some((u, s)) = self.texel_dummy() {
                            self.tables
                                .write_texel_addrs(&self.d, &self.layout, id, u, Some(s));
                        }
                    }
                }
                continue;
            }
            let Some(uv) = self.texel_view_persistent(sub, &t, false) else {
                continue;
            };
            let sv = self.texel_view_persistent(sub, &t, true);
            self.tables.write_texel_views(&self.d, id, uv, sv);
        }
        let mut imgs = self.images.lock().unwrap();
        // texture write-backs completed since the last submission (`tex_writeback`)
        let landing = std::mem::take(&mut *WB_LANDING.lock().unwrap());
        for w in &landing {
            self.tex_writeback_land(&mut imgs, w);
        }
        let pending = std::mem::take(&mut *IMG_PENDING.lock().unwrap());
        if !pending.is_empty() {
            let cb = sub.pre;
            self.upgrade_storage(&mut imgs, &pending, sub, cb);
        }
        let t_all = std::time::Instant::now();
        if !dirty.is_empty() && !imgs.by_addr.is_empty() {
            let max = imgs.max_size;
            let mut hit = Vec::new();
            let dirty_sorted = dirty.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0);
            let per_range: usize = if dirty_sorted {
                dirty.len().min(imgs.by_addr.len())
            } else {
                usize::MAX
            };
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
                        dirty[i0..]
                            .iter()
                            .take_while(|&&(a, _)| a < hi)
                            .copied()
                            .collect()
                    } else {
                        dirty
                            .iter()
                            .filter(|&&(a, n)| a < hi && a + n > lo)
                            .copied()
                            .collect()
                    };
                    let dirty = &mine[..];
                    let mut covered: u64 = dirty
                        .iter()
                        .map(|&(a, n)| (a + n).min(hi).saturating_sub(a.max(lo)))
                        .sum();
                    if im.gpu_owned && covered > 0 && !im.page_hash.is_empty() {
                        covered = super::perf::timed(super::perf::C::HashNs, || {
                            changed_page_coverage(&mut im.page_hash, lo, hi, dirty)
                        });
                    }
                    // a GPU-written texture is taken back from its storage only when the CPU rewrote most of it (a
                    // decoded movie frame); a write to a page it merely shares keeps the GPU's content
                    // bytes the texture already holds (a write to a page it shares with other data): nothing to upload
                    if covered > 0
                        && !im.gpu_owned
                        && !im.needs_upload
                        && im.sparse.is_none()
                        && !im.page_hash.is_empty()
                    {
                        let mut ph = std::mem::take(&mut im.page_hash);
                        let th = std::time::Instant::now();
                        let changed = super::perf::timed(super::perf::C::HashNs, || {
                            dirty.iter().fold(false, |c, &(a, n)| {
                                pages_changed(&mut ph, lo, hi, a, n) || c
                            })
                        });
                        let hms = th.elapsed().as_secs_f64() * 1e3;
                        if hms > 2.0 {
                            static SLOW: std::sync::atomic::AtomicU32 =
                                std::sync::atomic::AtomicU32::new(0);
                            if SLOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 10 {
                                let dn: u64 = dirty
                                    .iter()
                                    .map(|&(a, n)| (a + n).min(hi).saturating_sub(a.max(lo)))
                                    .sum();
                                crate::vklog!("texture hash check took {hms:.1} ms: ser{s} {}x{} fmt {:#x} at {lo:#x} ({} KB), {} dirty ranges over it ({} KB), {} page hashes", im.tex.d.width, im.tex.d.height, im.tex.d.format, (hi - lo) >> 10, dirty.len(), dn >> 10, ph.len());
                            }
                        }
                        im.page_hash = ph;
                        if !changed {
                            static SAID: std::sync::atomic::AtomicU32 =
                                std::sync::atomic::AtomicU32::new(0);
                            if im.rewrites > 30
                                && im.rewrites < 32
                                && SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 12
                            {
                                let d = &im.tex.d;
                                let dn: u64 = dirty
                                    .iter()
                                    .map(|&(a, n)| (a + n).min(hi).saturating_sub(a.max(lo)))
                                    .sum();
                                crate::vklog!("texture rewritten with the same bytes again and again: ser{s} {}x{}x{} fmt {:#x} flags {:#x} at {lo:#x} ({} KB), {} KB of it written this time in {} ranges", d.width, d.height, d.depth, d.format, d.flags, (hi - lo) >> 10, dn >> 10, dirty.len());
                            }
                            im.rewrites += 1;
                            super::perf::inc(super::perf::C::UploadsSame);
                            continue;
                        }
                    }
                    if covered > 0 && im.gpu_owned && covered * 2 < hi - lo {
                        // (diagnosis, lane K2: MKW's menu text misses letters) a CPU write the GPU-written texture keeps
                        static N: std::sync::atomic::AtomicU32 =
                            std::sync::atomic::AtomicU32::new(0);
                        if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 24 {
                            let d = &im.tex.d;
                            crate::vklog!("CPU write kept out of GPU-written texture ser{s} {}x{}x{} fmt {:#x} at {lo:#x}: {} of {} KB written", d.width, d.height, d.depth, d.format, covered >> 10, (hi - lo) >> 10);
                        }
                    }
                    if covered > 0 && (!im.gpu_owned || covered * 2 >= hi - lo) {
                        im.rewrites += 1;
                        // the CPU wrote the storage: its bytes are the texture's newest content, also when the GPU
                        // wrote the texture before (a movie frame the game decodes into a texture it cleared once:
                        // Mario Kart 8 Deluxe's mode-select preview stayed black)
                        if im.gpu_owned {
                            static SAID: std::sync::atomic::AtomicBool =
                                std::sync::atomic::AtomicBool::new(false);
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
            let Some((runs, gen)) = im.sparse.as_ref() else {
                continue;
            };
            if im.gpu_owned {
                continue;
            }
            let hit = dirty
                .iter()
                .any(|&(a, n)| runs.iter().any(|&(b, m)| a < b + m && b < a + n));
            if hit || (look && crate::nvn::vpool::gen_in(im.tex.mem as u64, im.tex.l.size) > *gen) {
                im.needs_upload = true;
            }
        }
        let serials: Vec<u64> = imgs
            .by_serial
            .iter()
            .filter(|(_, i)| i.needs_init || i.needs_upload)
            .map(|(s, _)| *s)
            .collect();
        // the textures' storage read and laid out linearly on several threads first (a streaming burst: Pokemon
        // Legends: Z-A uploads ~1,400 textures in a few seconds, 0.5 ms each on one thread: frames of 200-370 ms)
        let t_scan = t_all.elapsed().as_secs_f64();
        // page hashes of what is about to be read for the upload, taken before it is read: hashed after the upload
        // read the storage, a CPU write between the two (the game's font thread adding glyphs to MKW's 1024x8192 glyph
        // sheet) went into the hashes but not into the image, and its write-watch hit at the next submission found
        // the page "unchanged": the glyph was never uploaded (lane K3, the dropped menu letters)
        for &s in &serials {
            if let Some(im) = imgs.by_serial.get_mut(&s) {
                if im.needs_upload
                    && !im.gpu_owned
                    && !im.tex.mem.is_null()
                    && im.sparse.is_none()
                    && hash_upload(im)
                {
                    let lo = im.tex.mem as u64;
                    im.page_hash = page_hashes(lo, lo + im.tex.l.size);
                    im.hashed_early = true;
                }
            }
        }
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
        if imgs
            .by_serial
            .get(&serial)
            .is_none_or(|im| !im.needs_init && !im.needs_upload)
        {
            return;
        }
        self.prepare_image_in(imgs, serial, sub, 0);
        // An alias's upload initializes the shared backing too. Leaving the owner's pending upload set could
        // later overwrite an ordered upgrade (or a GPU write) with stale guest bytes.
        let Some(im) = imgs.by_serial.get(&serial) else {
            return;
        };
        let (image, at, size) = (im.image, im.tex.mem as u64, im.tex.l.size);
        let group = imgs.by_addr.get(&at).cloned().unwrap_or_default();
        let hashes = (group.len() > 1).then(|| im.page_hash.clone());
        for s in group {
            if let Some(other) = imgs.by_serial.get_mut(&s) {
                if other.image == image {
                    other.needs_init = false;
                    other.needs_upload = false;
                    if s != serial && other.tex.l.size == size {
                        if let Some(hashes) = &hashes {
                            other.page_hash.clone_from(hashes);
                        }
                    }
                }
            }
        }
    }

    /// `phase` 0: initialization and upload with their barriers; 1: initialization only, 2: upload only, both without
    /// the barrier after (the caller puts one after a whole batch: Mario Kart World's first submission initialized
    /// ~30,000 images, two full barriers each)
    fn prepare_image_in(&self, imgs: &mut Images, serial: u64, sub: &mut Sub, phase: u8) {
        let Some(im) = imgs.by_serial.get_mut(&serial) else {
            return;
        };
        let range = vk::ImageSubresourceRange {
            aspect_mask: aspect(&im.tf),
            base_mip_level: 0,
            level_count: im.levels,
            base_array_layer: 0,
            layer_count: im.layers,
        };
        if im.needs_init && phase != 2 {
            let b = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(im.image)
                .subresource_range(range)];
            unsafe {
                self.dev().cmd_pipeline_barrier2(
                    sub.pre,
                    &vk::DependencyInfo::default().image_memory_barriers(&b),
                )
            };
            // defined content (zero) for what no upload covers
            let is_ds = matches!(im.tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil);
            let compressed =
                im.bc7 || layout::fmt_bpb(im.tex.d.format).1 > 1 && im.tf.astc.is_none();
            if !compressed {
                if is_ds {
                    let v = vk::ClearDepthStencilValue {
                        depth: 0.0,
                        stencil: 0,
                    };
                    unsafe {
                        self.dev().cmd_clear_depth_stencil_image(
                            sub.pre,
                            im.image,
                            vk::ImageLayout::GENERAL,
                            &v,
                            &[range],
                        )
                    };
                } else {
                    let v = vk::ClearColorValue { uint32: [0; 4] };
                    unsafe {
                        self.dev().cmd_clear_color_image(
                            sub.pre,
                            im.image,
                            vk::ImageLayout::GENERAL,
                            &v,
                            &[range],
                        )
                    };
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
            // Hash before reading the upload, including the first content of images that the GPU can write.
            // image_begin_submit already took the baseline for its decode-ahead reads.
            if im.sparse.is_none() && hash_upload(im) && !std::mem::take(&mut im.hashed_early) {
                let lo = im.tex.mem as u64;
                im.page_hash = page_hashes(lo, lo + im.tex.l.size);
            }
            self.upload_from_storage(im, sub);
            let ns = t0.elapsed().as_nanos() as u64;
            super::perf::add(super::perf::C::TexUploadNs, ns);
            upload_stats::note(serial, &im.tex, ns);
            if ns > 500_000_000 {
                crate::vklog!(
                    "uploading texture ser{serial} ({}x{}x{} format {:#x}, {} MB) took {:.2} s",
                    im.tex.d.width,
                    im.tex.d.height,
                    im.tex.d.depth,
                    im.tex.d.format,
                    im.tex.l.size >> 20,
                    ns as f64 / 1e9
                );
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
        if gpu_deswizzle_ok(im) && self.record_gpu_deswizzle(im, sub, storage) {
            return;
        }
        let parts = decode_parts(im, storage);
        self.record_parts(im, sub, parts);
    }

    /// The storage staged as it is and deswizzled on the GPU (`tex::deswizzle_gpu`), every level and layer copied
    /// into the image from there. False (nothing recorded) when a level does not take that path.
    fn record_gpu_deswizzle(&self, im: &Image, sub: &mut Sub, storage: &[u8]) -> bool {
        let d = im.tex.d;
        let native = shape(&im.tex).1;
        let (sbuf, soff) = sub.stage_bytes_fresh(self, storage);
        let mut copies = Vec::new();
        for layer in 0..im.layers {
            for level in 0..im.levels {
                let gl = if d.target == target::T3D { 0 } else { layer };
                let Some((buf, off, _)) = crate::tex::deswizzle_gpu::record(
                    self,
                    sub,
                    &d,
                    level,
                    gl,
                    sbuf,
                    soff,
                    storage.len() as u64,
                ) else {
                    if copies.is_empty() && level == 0 && layer == 0 {
                        return false;
                    }
                    crate::log::red_once(&format!("texture {}x{} format {:#x}: level {level} layer {layer} not deswizzled on the GPU", d.width, d.height, d.format));
                    continue;
                };
                let (lw, lh, ld) = (
                    (native.width >> level).max(1),
                    (native.height >> level).max(1),
                    (native.depth >> level).max(1),
                );
                copies.push((
                    buf,
                    vk::BufferImageCopy {
                        buffer_offset: off,
                        buffer_row_length: 0,
                        buffer_image_height: 0,
                        image_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: upload_aspect(&im.tf),
                            mip_level: level,
                            base_array_layer: layer,
                            layer_count: 1,
                        },
                        image_offset: vk::Offset3D::default(),
                        image_extent: vk::Extent3D {
                            width: lw,
                            height: lh,
                            depth: if im.ty == vk::ImageType::TYPE_3D {
                                ld
                            } else {
                                1
                            },
                        },
                    },
                ));
            }
            if d.target == target::T3D {
                break;
            }
        }
        for (buf, r) in copies {
            copy_buf_img(self, sub.pre, im, buf, r, true);
        }
        GPU_DESWIZZLED.fetch_add(storage.len() as u64, std::sync::atomic::Ordering::Relaxed);
        true
    }

    fn record_parts(&self, im: &Image, sub: &mut Sub, parts: Vec<Part>) {
        let mut copies = Vec::new();
        for p in parts {
            if let Some((abw, abh, srgb)) = im.tf.astc {
                crate::tex::astc::record(
                    self, sub, &p.data, p.lw, p.lh, p.slices, abw, abh, srgb, im.bc7, im.image,
                    p.level, p.layer,
                );
                continue;
            }
            let (buf, off) = sub.stage_bytes(self, &p.data);
            copies.push((
                buf,
                vk::BufferImageCopy {
                    buffer_offset: off,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: upload_aspect(&im.tf),
                        mip_level: p.level,
                        base_array_layer: p.layer,
                        layer_count: 1,
                    },
                    image_offset: vk::Offset3D::default(),
                    image_extent: vk::Extent3D {
                        width: p.lw,
                        height: p.lh,
                        depth: if im.ty == vk::ImageType::TYPE_3D {
                            p.ld
                        } else {
                            1
                        },
                    },
                },
            ));
        }
        for (buf, r) in copies {
            copy_buf_img(self, sub.pre, im, buf, r, true);
        }
    }

    /// nvnTextureWriteTexels: into the storage (block-linear or linear) and into the image at the next submission.
    pub fn write_texels(
        &self,
        t: &tex_t,
        v: Option<view_t>,
        r: &NVNcopyRegion,
        data: *const u8,
        row: usize,
        img: usize,
    ) {
        // Turbo's write_texels: the texels go into the texture's storage and every image over that storage uploads
        // from it again. Written into the image alone they were lost to the next upload from the storage and never
        // reached another texture object over the same memory (Mario Kart 8 Deluxe's mode-select preview: the movie
        // decoder writes its frames into the client's texture, the menu samples the game's own object: black).
        let level = v
            .filter(|v| v.bits & VW_LEVELS != 0)
            .map(|v| v.base_level as u32)
            .unwrap_or(0);
        let layer0 = v
            .filter(|v| v.bits & VW_LAYERS != 0)
            .map(|v| v.min_layer as u32)
            .unwrap_or(0);
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
        let hit: Vec<u64> = imgs
            .by_addr
            .range(lo.saturating_sub(max)..lo + n)
            .flat_map(|(_, ss)| ss.iter().copied())
            .collect();
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
    /// `row` and `img` are byte pitches used literally, not zero-as-default sentinels. The destination
    /// is used synchronously and is not retained. Returns true only after copying completed data;
    /// failures are logged and leave the destination unchanged. A timed-out staging allocation stays
    /// owned by its submission until completion; no callback retains the destination pointer.
    ///
    /// # Safety
    /// With `(bpb, bw, bh) = layout::fmt_bpb(t.d.format)`, let
    /// `rb = ceil(max(r.width, 0) / bw) * bpb`, `rows = ceil(max(r.height, 0) / bh)` and
    /// `depth = max(r.depth, 1)`. These calculations, `rb * rows * depth`, and every destination
    /// offset/extent must be representable in `usize` and at most `isize::MAX` bytes.
    /// For every `z < depth` and `y < rows`, `dst.add(z * img + y * row)` must be within the same
    /// live allocation (or one-past for a zero-byte copy), valid for writing `rb` bytes. For nonempty
    /// rows this requires a span of at least `(depth - 1) * img + (rows - 1) * row + rb` bytes from
    /// `dst`, not merely `rb * rows * depth`. `dst` must be non-null even for zero-byte copies;
    /// only byte alignment is required, and destination bytes need not be initialized.
    /// The written ranges must not overlap the internal staging source or live shared references
    /// (including `self`, `t` and `r`), or be accessed concurrently until this call returns.
    /// Successive destination rows may overlap each other; copying is sequential. Pitches are not
    /// checked or adjusted here. The caller must also supply a valid texture/view/copy region.
    ///
    /// Even an owned destination does not make a raw-pointer call safe Rust:
    /// ```compile_fail,E0133
    /// use nxgfx::{nvn::{obj::tex_t, types::NVNcopyRegion}, vk::Gpu};
    /// fn read_pixel(g: &Gpu, t: &tex_t) {
    ///     assert_eq!(nxgfx::tex::layout::fmt_bpb(t.d.format), (4, 1, 1));
    ///     let r = NVNcopyRegion {
    ///         xoffset: 0, yoffset: 0, zoffset: 0, width: 1, height: 1, depth: 1,
    ///     };
    ///     let mut dst = [0u8; 4];
    ///     let _copied: bool = g.read_texels(t, None, &r, dst.as_mut_ptr(), 4, 4);
    /// }
    /// ```
    ///
    /// Positive compilation control for a valid RGBA8 source; no device is constructed or called:
    /// ```no_run
    /// use nxgfx::{nvn::{obj::tex_t, types::NVNcopyRegion}, vk::Gpu};
    /// fn read_pixel(g: &Gpu, t: &tex_t) {
    ///     assert_eq!(nxgfx::tex::layout::fmt_bpb(t.d.format), (4, 1, 1));
    ///     let r = NVNcopyRegion {
    ///         xoffset: 0, yoffset: 0, zoffset: 0, width: 1, height: 1, depth: 1,
    ///     };
    ///     let mut dst = [0u8; 4];
    ///     // SAFETY: for the valid source, one four-byte row fits this exclusively owned array;
    ///     // it stays live for the synchronous call and is separate from the staging allocation.
    ///     let _copied: bool = unsafe { g.read_texels(t, None, &r, dst.as_mut_ptr(), 4, 4) };
    /// }
    /// ```
    #[must_use]
    pub unsafe fn read_texels(
        &self,
        t: &tex_t,
        v: Option<view_t>,
        r: &NVNcopyRegion,
        dst: *mut u8,
        row: usize,
        img: usize,
    ) -> bool {
        let level = v
            .filter(|v| v.bits & VW_LEVELS != 0)
            .map(|v| v.base_level as u32)
            .unwrap_or(0);
        let layer0 = v
            .filter(|v| v.bits & VW_LAYERS != 0)
            .map(|v| v.min_layer as u32)
            .unwrap_or(0);
        let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
        let rb = (r.width.max(0) as u32).div_ceil(bw) as usize * bpb as usize;
        let rows = (r.height.max(0) as u32).div_ceil(bh) as usize;
        let depth = r.depth.max(1) as usize;
        let total = (rb * rows * depth) as u64;
        let rbuf = match self.buffer(
            total,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
            "read texels",
        ) {
            Ok(b) => b,
            Err(e) => {
                crate::log::red(&format!(
                    "ReadTexels: staging allocation failed: {e}; destination unchanged"
                ));
                return false;
            }
        };
        let staging = Arc::new(Mutex::new(ReadbackState::new(rbuf)));
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
                    base_array_layer: if is3d {
                        layer0
                    } else {
                        layer0 + r.zoffset as u32
                    },
                    layer_count: if is3d { 1 } else { depth as u32 },
                },
                image_offset: vk::Offset3D {
                    x: r.xoffset,
                    y: r.yoffset,
                    z: if is3d { r.zoffset } else { 0 },
                },
                image_extent: vk::Extent3D {
                    width: r.width as u32,
                    height: r.height as u32,
                    depth: if is3d { depth as u32 } else { 1 },
                },
            };
            let buf = staging.lock().unwrap().buffer.as_ref().unwrap().buf;
            if record_copy_buf_img(g, sub.main, im, buf, reg, false) {
                staging.lock().unwrap().recorded = true;
                let completed = Arc::clone(&staging);
                // This callback belongs to the exact submission that references buf. It carries no
                // guest destination, and can release an abandoned buffer only after GPU completion.
                sub.post
                    .push(super::queue::Post::Callback(Box::new(move |g| {
                        complete_readback(&completed, |b| g.free_buffer(b));
                    })));
            }
        });
        let result = finish_readback(
            &staging,
            || super::queue::wait_fence(f, u64::MAX),
            |rbuf| {
                for z in 0..depth {
                    for y in 0..rows {
                        // SAFETY: finish_readback requires both successful wait and this submission's
                        // completion callback before exposing the source; the caller guarantees the
                        // pitched destination extent, nonoverlap and representable byte arithmetic.
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                rbuf.ptr.add((z * rows + y) * rb),
                                dst.add(z * img + y * row),
                                rb,
                            )
                        };
                    }
                }
            },
            |b| self.free_buffer(b),
        );
        if let Err(reason) = result {
            let why = match reason {
                ReadbackFailure::NotRecorded => "no image copy was recorded",
                ReadbackFailure::NotCompleted => {
                    "completion was not confirmed; staging retires only after GPU completion"
                }
            };
            crate::log::red(&format!(
                "ReadTexels: submission {f}: {why}; destination unchanged"
            ));
        }
        result.is_ok()
    }
}

/// The caller and the submission's Post callback share ownership, never the destination pointer.
/// `abandoned` also covers a finished caller: completion may take the buffer only after that point.
struct ReadbackState<B> {
    buffer: Option<B>,
    recorded: bool,
    completed: bool,
    abandoned: bool,
}

impl<B> ReadbackState<B> {
    fn new(buffer: B) -> Self {
        Self {
            buffer: Some(buffer),
            recorded: false,
            completed: false,
            abandoned: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadbackFailure {
    NotRecorded,
    NotCompleted,
}

/// Called only by the Post callback attached to the submission containing the copy.
fn complete_readback<B>(state: &Mutex<ReadbackState<B>>, release: impl FnOnce(B)) {
    let buffer = {
        let mut s = state.lock().unwrap();
        s.completed = true;
        if s.abandoned {
            s.buffer.take()
        } else {
            None
        }
    };
    if let Some(buffer) = buffer {
        release(buffer);
    }
}

/// Production failure flow, with CPU-injectable wait/copy/release operations. Never wait with the
/// state locked: the submission callback must be able to publish completion before wait returns.
fn finish_readback<B>(
    state: &Mutex<ReadbackState<B>>,
    wait: impl FnOnce() -> bool,
    copy: impl FnOnce(&B),
    release: impl FnOnce(B),
) -> Result<(), ReadbackFailure> {
    let recorded = state.lock().unwrap().recorded;
    let waited = recorded && wait();
    let (buffer, result) = {
        let mut s = state.lock().unwrap();
        s.abandoned = true;
        let result = if !recorded {
            Err(ReadbackFailure::NotRecorded)
        } else if !waited || !s.completed || s.buffer.is_none() {
            Err(ReadbackFailure::NotCompleted)
        } else {
            Ok(())
        };
        // With no recorded copy the buffer was never referenced by the submission. Otherwise only
        // the exact completion callback can prove it safe to take, including the timeout race.
        let buffer = if !recorded || s.completed {
            s.buffer.take()
        } else {
            None
        };
        (buffer, result)
    };
    if let Some(buffer) = buffer {
        if result.is_ok() {
            copy(&buffer);
        }
        release(buffer);
    }
    result
}

#[cfg(test)]
mod readback_tests {
    use super::{complete_readback, finish_readback, ReadbackFailure, ReadbackState};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    struct Buffer(Rc<RefCell<Vec<&'static str>>>);
    impl Drop for Buffer {
        fn drop(&mut self) {
            self.0.borrow_mut().push("drop");
        }
    }

    fn pending(events: &Rc<RefCell<Vec<&'static str>>>) -> Arc<Mutex<ReadbackState<Buffer>>> {
        let mut s = ReadbackState::new(Buffer(Rc::clone(events)));
        s.recorded = true;
        Arc::new(Mutex::new(s))
    }

    fn release(b: Buffer) {
        b.0.borrow_mut().push("release");
    }

    #[test]
    fn failed_wait_retains_staging_until_its_own_completion() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let state = pending(&events);
        let callback = {
            let owned = Arc::clone(&state);
            move || complete_readback(&owned, release)
        };
        let mut dst = [0xa5; 4];
        let result = finish_readback(&state, || false, |_| dst.fill(0), release);
        assert_eq!(result, Err(ReadbackFailure::NotCompleted));
        assert_eq!(dst, [0xa5; 4]);
        assert!(
            events.borrow().is_empty(),
            "pending staging was read/released/dropped"
        );
        // Another submission completing cannot retire this buffer.
        let other_events = Rc::new(RefCell::new(Vec::new()));
        let other = pending(&other_events);
        complete_readback(&other, release);
        assert!(events.borrow().is_empty());
        drop(state); // the returning caller no longer owns a reference; the queued callback does
        assert!(events.borrow().is_empty());
        callback();
        assert_eq!(*events.borrow(), ["release", "drop"]);
        assert_eq!(
            dst, [0xa5; 4],
            "retirement must never write a former caller's destination"
        );
    }

    #[test]
    fn successful_completion_copies_before_release() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let state = pending(&events);
        let mut dst = [0xa5; 4];
        let result = finish_readback(
            &state,
            || {
                complete_readback(&state, release);
                assert!(events.borrow().is_empty(), "callback freed a wanted source");
                true
            },
            |_| {
                events.borrow_mut().push("copy");
                dst.copy_from_slice(&[1, 2, 3, 4]);
            },
            release,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(dst, [1, 2, 3, 4]);
        assert_eq!(*events.borrow(), ["copy", "release", "drop"]);
        complete_readback(&state, release);
        assert_eq!(
            *events.borrow(),
            ["copy", "release", "drop"],
            "no double release"
        );
    }

    #[test]
    fn no_recorded_copy_does_not_wait_or_touch_destination() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let state = Mutex::new(ReadbackState::new(Buffer(Rc::clone(&events))));
        let mut dst = [0xa5; 4];
        let result = finish_readback(
            &state,
            || panic!("no command references this allocation"),
            |_| dst.fill(0),
            release,
        );
        assert_eq!(result, Err(ReadbackFailure::NotRecorded));
        assert_eq!(dst, [0xa5; 4]);
        assert_eq!(*events.borrow(), ["release", "drop"]);
    }

    #[test]
    fn timeout_racing_completion_releases_without_copying() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let state = pending(&events);
        let result = finish_readback(
            &state,
            || {
                complete_readback(&state, release);
                false // wait failed even though the callback has now run
            },
            |_| panic!("a failed wait must not consume staging"),
            release,
        );
        assert_eq!(result, Err(ReadbackFailure::NotCompleted));
        assert_eq!(*events.borrow(), ["release", "drop"]);
    }

    #[test]
    fn wait_success_without_matching_callback_is_not_completion() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let state = pending(&events);
        assert_eq!(
            finish_readback(
                &state,
                || true,
                |_| panic!("copy before completion"),
                release
            ),
            Err(ReadbackFailure::NotCompleted)
        );
        assert!(events.borrow().is_empty());
        complete_readback(&state, release);
        assert_eq!(*events.borrow(), ["release", "drop"]);
    }
}

/// A buffer <-> image copy (`to_image`: into the image), its region in the texture's texels: through a bounce image
/// of the texture's size when the image is scaled (resolution scale).
pub fn copy_buf_img(
    g: &Gpu,
    cb: vk::CommandBuffer,
    im: &Image,
    buf: vk::Buffer,
    r: vk::BufferImageCopy,
    to_image: bool,
) {
    record_copy_buf_img(g, cb, im, buf, r, to_image);
}

/// True only when a buffer/image copy was recorded; preserve the public void helper for other callers.
fn record_copy_buf_img(
    g: &Gpu,
    cb: vk::CommandBuffer,
    im: &Image,
    buf: vk::Buffer,
    r: vk::BufferImageCopy,
    to_image: bool,
) -> bool {
    let dev = g.dev();
    super::lost::checkpoint(g, cb, true, || {
        format!(
            "a copy {} texture ser{} ({}x{}) {} buffer {:#x} at {:#x}",
            if to_image { "into" } else { "out of" },
            im.tex.serial,
            r.image_extent.width,
            r.image_extent.height,
            if to_image { "from" } else { "to" },
            vk::Handle::as_raw(buf),
            r.buffer_offset
        )
    });
    if im.scale == 1.0 {
        unsafe {
            if to_image {
                dev.cmd_copy_buffer_to_image(cb, buf, im.image, vk::ImageLayout::GENERAL, &[r]);
            } else {
                dev.cmd_copy_image_to_buffer(cb, im.image, vk::ImageLayout::GENERAL, buf, &[r]);
            }
        }
        return true;
    }
    let sl = r.image_subresource;
    let e = vk::Extent3D {
        width: r.image_extent.width,
        height: r.image_extent.height,
        depth: 1,
    };
    let Some(b) = super::scale::bounce(g, im.format, e, sl.layer_count) else {
        crate::log::red_once(
            "resolution scale: no bounce image for a copy of a scaled render target: not copied",
        );
        return false;
    };
    let bl = vk::ImageSubresourceLayers {
        aspect_mask: sl.aspect_mask,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: sl.layer_count,
    };
    let br = vk::BufferImageCopy {
        image_subresource: bl,
        image_offset: vk::Offset3D::default(),
        image_extent: e,
        ..r
    };
    let flt = super::scale::filter(g, im.format, im.tf.kind);
    let asp = aspect(&im.tf);
    super::scale::general(g, cb, b, asp, sl.layer_count, true);
    unsafe {
        if to_image {
            dev.cmd_copy_buffer_to_image(cb, buf, b, vk::ImageLayout::GENERAL, &[br]);
            super::scale::general(g, cb, b, asp, sl.layer_count, false);
            super::scale::blit(
                g,
                cb,
                b,
                1.0,
                bl,
                vk::Offset3D::default(),
                im.image,
                im.scale,
                sl,
                r.image_offset,
                e,
                flt,
            );
        } else {
            super::scale::blit(
                g,
                cb,
                im.image,
                im.scale,
                sl,
                r.image_offset,
                b,
                1.0,
                bl,
                vk::Offset3D::default(),
                e,
                flt,
            );
            super::scale::general(g, cb, b, asp, sl.layer_count, false);
            dev.cmd_copy_image_to_buffer(cb, b, vk::ImageLayout::GENERAL, buf, &[br]);
        }
    }
    true
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
        out[i * 4 * comp..i * 4 * comp + 3 * comp]
            .copy_from_slice(&src[i * 3 * comp..(i + 1) * 3 * comp]);
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
            (
                format!(
                    "ser{serial} {}x{}x{} fmt {:#x} flags {:#x} at {:#x} ({} KB)",
                    t.d.width,
                    t.d.height,
                    t.d.depth,
                    t.d.format,
                    t.d.flags,
                    t.mem as u64,
                    t.l.size / 1024
                ),
                0,
                0,
            )
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
        let top: Vec<String> = v
            .iter()
            .take(6)
            .map(|e| {
                format!(
                    "{} x{:.2}/frame {:.2} ms/frame",
                    e.0,
                    e.1 as f64 / f,
                    e.2 as f64 / 1e6 / f
                )
            })
            .collect();
        Some(format!(
            "uploads from storage: {} textures, {:.2} ms per frame; most: {}",
            v.len(),
            total as f64 / 1e6 / f,
            top.join("; ")
        ))
    }
}

/// WriteTexels' bytes (rows of `row` bytes, images of `img`) into a texture's storage: block-linear levels through
/// the swizzle (a level-layer the region does not cover whole is read first), linear ones row by row at its stride.
fn write_storage(
    t: &tex_t,
    level: u32,
    layer0: u32,
    r: &NVNcopyRegion,
    data: *const u8,
    row: usize,
    img: usize,
) -> Option<()> {
    let (bpb, bw, bh) = layout::fmt_bpb(t.d.format);
    if bpb == 0 || r.xoffset < 0 || r.yoffset < 0 || r.zoffset < 0 || r.width <= 0 || r.height <= 0
    {
        return None;
    }
    let rb = (r.width as u32).div_ceil(bw) as usize * bpb as usize;
    let rows = (r.height as u32).div_ceil(bh) as usize;
    let (x0, y0) = (
        (r.xoffset as u32 / bw * bpb) as usize,
        (r.yoffset as u32 / bh) as usize,
    );
    let depth = r.depth.max(1) as usize;
    let is3d = t.d.target == target::T3D;
    // SAFETY: the texture's storage is guest memory of l.size bytes the game keeps while the texture is live
    let storage = unsafe { core::slice::from_raw_parts_mut(t.mem, t.l.size as usize) };
    if t.d.flags as u32 & tex_flags::LINEAR != 0 {
        let stride = if t.d.stride != 0 {
            t.d.stride as usize
        } else {
            (t.d.width.div_ceil(bw) * bpb) as usize
        };
        if level != 0 || depth != 1 || x0 + rb > stride {
            return None;
        }
        let base = layer0 as usize * t.l.layer_size as usize;
        for y in 0..rows {
            let o = base + (y0 + y) * stride + x0;
            if o + rb > storage.len() {
                return None;
            }
            storage[o..o + rb]
                .copy_from_slice(unsafe { core::slice::from_raw_parts(data.add(y * row), rb) });
        }
        return Some(());
    }
    // block-linear: one level-layer (3D: all its slices) at a time
    let groups: Vec<(u32, Vec<usize>)> = if is3d {
        vec![(layer0, (0..depth).collect())]
    } else {
        (0..depth)
            .map(|z| (layer0 + r.zoffset as u32 + z as u32, vec![z]))
            .collect()
    };
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
        let whole = x0 == 0
            && rb == pitch
            && y0 == 0
            && rows == g.rows as usize
            && zs.len() == g.slices as usize;
        if !whole {
            deswizzle::deswizzle(&t.d, level, layer, storage, &mut lin, pitch)?;
        }
        for &z in &zs {
            let slice = if is3d { z_first + z } else { 0 };
            for y in 0..rows {
                let o = (slice * g.rows as usize + y0 + y) * pitch + x0;
                lin[o..o + rb].copy_from_slice(unsafe {
                    core::slice::from_raw_parts(data.add(z * img + y * row), rb)
                });
            }
        }
        deswizzle::swizzle(&t.d, level, layer, &lin, pitch, storage)?;
    }
    Some(())
}

/// Texture registrations waiting for their image (`Gpu::reg_tex`).
mod lazy {
    use crate::nvn::obj::{tex_t, view_t};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct L {
        by_id: HashMap<i32, (tex_t, Option<view_t>)>,
        by_serial: HashMap<u64, Vec<i32>>,
        /// texture objects finalized while registrations of them still wait (`finalized`)
        gone: std::collections::HashSet<u64>,
    }
    static ST: Mutex<Option<L>> = Mutex::new(None);
    static N: AtomicUsize = AtomicUsize::new(0);

    pub fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("NXGFX_LAZY_IMAGES").as_deref() != Ok("0"))
    }
    pub fn put(id: i32, t: tex_t, v: Option<view_t>) {
        let mut g = ST.lock().unwrap();
        let l = g.get_or_insert_with(L::default);
        l.by_serial.entry(t.serial).or_default().push(id);
        if l.by_id.insert(id, (t, v)).is_none() {
            N.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// The registration waiting at `id`, and whether its texture object was finalized since (`finalized`).
    pub fn take(id: i32) -> Option<(tex_t, Option<view_t>, bool)> {
        if N.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let mut g = ST.lock().unwrap();
        let l = g.as_mut()?;
        let (t, v) = l.by_id.remove(&id)?;
        N.fetch_sub(1, Ordering::Relaxed);
        let gone = l.gone.contains(&t.serial);
        if let Some(ids) = l.by_serial.get_mut(&t.serial) {
            ids.retain(|&i| i != id);
            if ids.is_empty() {
                l.by_serial.remove(&t.serial);
                l.gone.remove(&t.serial);
            }
        }
        Some((t, v, gone))
    }
    pub fn forget_id(id: i32) {
        let _ = take(id);
    }
    /// Whether a finalized texture's waiting registrations stay (`NXGFX_LAZY_KEEP=0`: dropped, as before).
    fn keep() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("NXGFX_LAZY_KEEP").as_deref() != Ok("0"))
    }
    /// nvnTextureFinalize of texture object `serial`: its pool slots still name its storage on the console (the
    /// descriptor outlives the object), so registrations of it that wait for their image stay, marked finalized: the
    /// first use makes the image and lets it go after that submission, as an image made at registration would have
    /// been parked. Mario Kart 8 Deluxe's menus register and finalize thousands a second; dropped, every later draw
    /// read the zero stand-in.
    pub fn finalized(serial: u64) {
        if N.load(Ordering::Relaxed) == 0 {
            return;
        }
        let mut g = ST.lock().unwrap();
        let Some(l) = g.as_mut() else { return };
        if !l.by_serial.contains_key(&serial) {
            return;
        }
        if keep() {
            l.gone.insert(serial);
            return;
        }
        for id in l.by_serial.remove(&serial).unwrap_or_default() {
            if l.by_id.get(&id).is_some_and(|(t, _)| t.serial == serial) {
                l.by_id.remove(&id);
                N.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
    pub fn take_all() -> Vec<(i32, (tex_t, Option<view_t>, bool))> {
        if N.load(Ordering::Relaxed) == 0 {
            return Vec::new();
        }
        let mut g = ST.lock().unwrap();
        let Some(l) = g.as_mut() else {
            return Vec::new();
        };
        N.store(0, Ordering::Relaxed);
        l.by_serial.clear();
        let gone = std::mem::take(&mut l.gone);
        l.by_id
            .drain()
            .map(|(id, (t, v))| (id, (t, v, gone.contains(&t.serial))))
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn tex(serial: u64) -> tex_t {
            // SAFETY: tex_t is plain data (repr(C) integers and a raw pointer); all zero is a valid value.
            let mut t: tex_t = unsafe { std::mem::zeroed() };
            t.serial = serial;
            t
        }

        /// A registration waiting for its image survives its texture object's finalize, marked finalized, and is
        /// taken once; one never finalized is not marked.
        #[test]
        fn a_finalized_textures_waiting_registration_stays_marked() {
            put(910_001, tex(0xF1_0001), None);
            put(910_002, tex(0xF1_0001), None);
            put(910_003, tex(0xF1_0002), None);
            finalized(0xF1_0001);
            assert!(matches!(take(910_001), Some((t, None, true)) if t.serial == 0xF1_0001));
            assert!(matches!(take(910_002), Some((_, _, true))));
            assert!(take(910_001).is_none(), "taken once");
            assert!(matches!(take(910_003), Some((_, _, false))), "not finalized");
            put(910_004, tex(0xF1_0001), None);
            assert!(matches!(take(910_004), Some((_, _, false))), "the mark went with its last registration");
        }
    }
    /// Registrations waiting (the memory line).
    pub fn waiting() -> usize {
        N.load(Ordering::Relaxed)
    }
}
pub fn lazy_waiting() -> usize {
    lazy::waiting()
}

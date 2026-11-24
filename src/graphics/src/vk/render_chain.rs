//! One dump frame's attachment observations joined to actual sampled/storage views. CPU metadata,
//! not proof of shader writes. Optional raw readbacks are reserved before any allocation/command.
use crate::vk::image::{Image, ImageCreation, ResolvedView};
use ash::vk;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

const RECORDS: usize = 256;
const EVENTS: usize = 16_384;
const SAMPLES: usize = 16;
const BYTES: u64 = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub frame: u64,
    pub submission: u64,
    pub pass: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub owner: u64,
    pub creation_id: u64,
    pub image: u64,
    pub view: u64,
    pub writes: u64,
    pub width: u32,
    pub height: u32,
    /// Immutable VkImage creation format, independent of the texture object currently owning it.
    pub raw_format: i32,
    pub registered_format: i32,
    /// Final successful view creation input (format, aspect, range, type and swizzle), if retained.
    pub interpretation: Option<ResolvedView>,
    pub aspect: u32,
    pub bpp: u32,
}

impl Resource {
    // Known resources cover the same single subresource. View format/swizzle can differ without
    // changing the backing; both interpretations remain in the producer and consumer records.
    pub(crate) fn same_image(self, other: Self) -> bool {
        self.image != 0
            && self.creation_id != 0
            && self.creation_id == other.creation_id
            && self.aspect != 0
            && self.owner == other.owner
            && self.image == other.image
            && self.aspect == other.aspect
            && self.width == other.width
            && self.height == other.height
            && self.raw_format == other.raw_format
    }

    fn bytes(self) -> Option<u64> {
        if self.width == 0 || self.height == 0 || self.bpp == 0 {
            return None;
        }
        u64::from(self.width)
            .checked_mul(u64::from(self.height))?
            .checked_mul(u64::from(self.bpp))
    }

    pub fn raw_filename(self, at: Point, slot: usize) -> String {
        format!(
            "chain-{}-sub{}-pass{}-slot{}-image{:x}-{}x{}-fmt{}.bin",
            at.frame,
            at.submission,
            at.pass,
            slot,
            self.image,
            self.width,
            self.height,
            self.raw_format
        )
    }
}

/// CPU input to the actual diagnostic resolver. Both registered aliases and canonical owners are
/// described; knowing the alias's view handle does not establish a canonical copy format.
#[derive(Clone, Copy)]
pub struct ImageInfo {
    pub image: u64,
    pub creation_id: u64,
    pub format: i32,
    pub extent: [u32; 3],
    pub image_type: i32,
    pub levels: u32,
    pub layers: u32,
    pub samples: u32,
}

impl ImageInfo {
    pub fn of(im: &Image) -> Self {
        Self {
            image: vk::Handle::as_raw(im.image),
            creation_id: im.backing_creation().map_or(0, |c| c.id),
            format: im.format.as_raw(),
            extent: [im.extent.width, im.extent.height, im.extent.depth],
            image_type: im.ty.as_raw(),
            levels: im.levels,
            layers: im.layers,
            samples: im.samples.as_raw(),
        }
    }

    fn from_creation(c: ImageCreation) -> Self {
        Self {
            image: c.image,
            creation_id: c.id,
            format: c.format,
            extent: c.extent,
            image_type: c.image_type,
            levels: c.levels,
            layers: c.layers,
            samples: c.samples,
        }
    }

    fn backing(im: &Image) -> Self {
        im.backing_creation()
            .map(Self::from_creation)
            .unwrap_or(Self {
                image: 0,
                creation_id: 0,
                format: vk::Format::UNDEFINED.as_raw(),
                ..Self::of(im)
            })
    }

    fn single_2d(self) -> bool {
        self.image_type == vk::ImageType::TYPE_2D.as_raw()
            && self.extent[2] == 1
            && self.levels == 1
            && self.layers == 1
            && self.samples == vk::SampleCountFlags::TYPE_1.as_raw()
    }
}

/// Shared by chain_resource and the CPU owner-promotion regression: never infer physical creation
/// information from the promoted texture owner's registered/rendering format.
pub fn resolve_images(
    owner: &Image,
    registered: &Image,
    view: u64,
    actual: Option<ResolvedView>,
) -> Resource {
    resolve_resource(
        owner.tex.serial,
        owner.gpu_seq,
        ImageInfo::backing(owner),
        ImageInfo::of(registered),
        view,
        actual,
    )
}

fn raw_layout(format: i32) -> Option<(u32, u32)> {
    let (bpp, aspect) = match vk::Format::from_raw(format) {
        vk::Format::D32_SFLOAT => (4, vk::ImageAspectFlags::DEPTH),
        vk::Format::R8_UNORM => (1, vk::ImageAspectFlags::COLOR),
        vk::Format::R16_SFLOAT | vk::Format::R8G8_UNORM => (2, vk::ImageAspectFlags::COLOR),
        vk::Format::R32_SFLOAT
        | vk::Format::R8G8B8A8_UNORM
        | vk::Format::R8G8B8A8_SRGB
        | vk::Format::B8G8R8A8_UNORM
        | vk::Format::B8G8R8A8_SRGB
        | vk::Format::B10G11R11_UFLOAT_PACK32 => (4, vk::ImageAspectFlags::COLOR),
        _ => return None,
    };
    Some((bpp, aspect.as_raw()))
}

/// Canonical raw bytes and separately preserved effective interpretation. A final-create metadata
/// miss (including eviction), unsupported format or unverified range yields no copy reservation.
pub fn resolve_resource(
    owner: u64,
    writes: u64,
    backing: ImageInfo,
    registered: ImageInfo,
    view: u64,
    actual: Option<ResolvedView>,
) -> Resource {
    let actual = actual.filter(|v| v.view == view && view != 0 && v.image == backing.image);
    let layout = raw_layout(backing.format);
    let known = backing.image != 0
        && backing.creation_id != 0
        && backing.creation_id == registered.creation_id
        && backing.image == registered.image
        && backing.single_2d()
        && registered.single_2d()
        && backing.extent == registered.extent
        && actual.is_some_and(|v| {
            layout.is_some()
                && raw_layout(v.format) == layout
                && Some(v.aspect) == layout.map(|(_, a)| a)
                && v.base_level == 0
                && v.levels == 1
                && v.base_layer == 0
                && v.layers == 1
                && (v.view_type == vk::ImageViewType::TYPE_2D.as_raw()
                    || v.view_type == vk::ImageViewType::TYPE_2D_ARRAY.as_raw())
        });
    Resource {
        owner,
        creation_id: backing.creation_id,
        image: if known { backing.image } else { 0 },
        view,
        writes,
        width: backing.extent[0],
        height: backing.extent[1],
        raw_format: backing.format,
        registered_format: registered.format,
        interpretation: actual,
        aspect: if known {
            layout.map_or(0, |(_, a)| a)
        } else {
            0
        },
        bpp: if known {
            layout.map_or(0, |(n, _)| n)
        } else {
            0
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    NotSelected,
    BudgetOrExtent,
    AllocationFailed,
    Recorded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub at: Point,
    pub slot: usize,
    pub resource: Resource,
    pub sample: Sample,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Observation(Observation),
    ChangedSince(Observation),
    Unknown,
}

#[derive(Default)]
struct Ledger {
    frame: u64,
    events: usize,
    samples: usize,
    bytes: u64,
    evicted: usize,
    observations: VecDeque<Observation>,
}

impl Ledger {
    fn admit(&mut self, at: Point) -> bool {
        if self.frame == 0 {
            self.frame = at.frame;
        }
        if at.frame == 0 || at.frame != self.frame || self.events >= EVENTS {
            return false;
        }
        self.events += 1;
        true
    }

    fn reserve(&mut self, resource: Resource) -> Option<u64> {
        let n = resource.bytes()?;
        let total = self.bytes.checked_add(n)?;
        if self.samples >= SAMPLES || total > BYTES {
            return None;
        }
        self.samples += 1;
        self.bytes = total;
        Some(n)
    }

    fn producer(&mut self, observation: Observation) {
        if self.observations.len() == RECORDS {
            self.observations.pop_front();
            self.evicted += 1;
        }
        self.observations.push_back(observation);
    }

    fn consumer(&self, at: Point, resource: Resource) -> Link {
        match self
            .observations
            .iter()
            .rev()
            .find(|o| o.at.frame == at.frame && o.resource.same_image(resource))
        {
            Some(o) if o.resource.writes == resource.writes => Link::Observation(*o),
            Some(o) => Link::ChangedSince(*o),
            None => Link::Unknown,
        }
    }
}

struct Config {
    extents: Vec<(u32, u32)>,
    passes: Vec<u64>,
}

fn config() -> Option<&'static Config> {
    static C: OnceLock<Option<Config>> = OnceLock::new();
    C.get_or_init(|| {
        if std::env::var("NXGFX_RENDER_CHAIN").as_deref() != Ok("1") {
            return None;
        }
        let parsed = (|| {
            let raw = std::env::var("NXGFX_CHAIN_EXTENTS").ok()?;
            if raw.len() > 128 { return None; }
            let extents: Option<Vec<_>> = raw.split(',').map(|s| {
                let (w, h) = s.trim().split_once('x')?;
                let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
                ((1..=16384).contains(&w) && (1..=16384).contains(&h)).then_some((w, h))
            }).collect();
            let extents = extents?;
            let raw = std::env::var("NXGFX_CHAIN_PASSES").unwrap_or_default();
            if raw.len() > 256 { return None; }
            let passes: Option<Vec<u64>> = raw.split(',').filter(|s| !s.is_empty())
                .map(|s| s.trim().parse().ok()).collect();
            let passes = passes?;
            (extents.len() <= 4 && passes.len() <= 16).then_some(Config { extents, passes })
        })();
        if parsed.is_none() {
            crate::log::red("render chain refused: require up to four positive NXGFX_CHAIN_EXTENTS=WxH entries and at most 16 NXGFX_CHAIN_PASSES");
        }
        parsed
    }).as_ref()
}

static STATE: OnceLock<Mutex<Ledger>> = OnceLock::new();

pub fn enabled() -> bool {
    config().is_some()
}

pub fn wanted(resource: Resource) -> bool {
    config().is_some_and(|c| c.extents.contains(&(resource.width, resource.height)))
}

/// Returns None when inactive/capped. Reservation is conservative: allocation failures do not refund
/// the session budget. No new frame can replenish it while old submissions still own readbacks.
pub fn reserve(at: Point, resource: Resource) -> Option<(Sample, u64)> {
    let c = config()?;
    if !wanted(resource) {
        return None;
    }
    let mut s = STATE.get_or_init(Default::default).lock().unwrap();
    if !s.admit(at) {
        return None;
    }
    if s.events == EVENTS {
        crate::vklog!(
            "render chain: metadata event cap reached; subsequent observations are unavailable"
        );
    }
    Some(if !c.passes.contains(&at.pass) {
        (Sample::NotSelected, 0)
    } else {
        s.reserve(resource)
            .map_or((Sample::BudgetOrExtent, 0), |n| (Sample::Recorded, n))
    })
}

pub fn producer(observation: Observation) {
    let mut s = STATE.get_or_init(Default::default).lock().unwrap();
    s.producer(observation);
    crate::vklog!("render chain attachment {observation:?}; history evictions {}; Recorded means queued, not completed or pixels written", s.evicted);
}

pub fn consumer(
    at: Point,
    stage: u32,
    shader: [u8; 8],
    binding: u32,
    storage: bool,
    resource: Resource,
) {
    if !wanted(resource) {
        return;
    }
    let mut s = STATE.get_or_init(Default::default).lock().unwrap();
    if !s.admit(at) {
        return;
    }
    let prior = s.consumer(at, resource);
    crate::vklog!("render chain binding at {at:?} stage {stage} shader {} binding {binding} storage {storage} resource {resource:?}: {prior:?}; association only, not proof of execution or latest writer", crate::nvn::prog::key_hex(&shader));
    if s.events == EVENTS {
        crate::vklog!(
            "render chain: metadata event cap reached; subsequent bindings are unavailable"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource() -> Resource {
        let im = image(vk::Format::B10G11R11_UFLOAT_PACK32);
        resolve_resource(7, 4, im, im, 101, Some(view(101, im, im.format)))
    }

    fn image(format: vk::Format) -> ImageInfo {
        let ci = vk::ImageCreateInfo::default()
            .format(format)
            .extent(vk::Extent3D {
                width: 1440,
                height: 810,
                depth: 1,
            })
            .image_type(vk::ImageType::TYPE_2D)
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1);
        ImageInfo::from_creation(ImageCreation::from_create_info(
            <vk::Image as vk::Handle>::from_raw(100),
            &ci,
        ))
    }

    fn view(handle: u64, image: ImageInfo, format: i32) -> ResolvedView {
        // The same metadata constructor used on the successful final vkCreateImageView input.
        let ci = vk::ImageViewCreateInfo::default()
            .image(<vk::Image as vk::Handle>::from_raw(image.image))
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::from_raw(format))
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::from_raw(
                        raw_layout(image.format).unwrap().1,
                    ))
                    .level_count(1)
                    .layer_count(1),
            );
        ResolvedView::from_create_info(<vk::ImageView as vk::Handle>::from_raw(handle), &ci)
    }
    fn point(frame: u64, submission: u64, pass: u64) -> Point {
        Point {
            frame,
            submission,
            pass,
        }
    }

    #[test]
    fn chain_resource_preserves_backing_and_effective_view_formats() {
        let backing = image(vk::Format::R8G8B8A8_UNORM); // owner 7, image H=100, format 37
        let alias = ImageInfo {
            format: vk::Format::B8G8R8A8_UNORM.as_raw(),
            ..backing
        }; // serial 8
        let resolve = |registered, handle, actual| {
            resolve_resource(7, 4, backing, registered, handle, actual)
        };
        let alias_attachment = resolve(alias, 501, Some(view(501, alias, alias.format)));
        let owner_sample = resolve(backing, 502, Some(view(502, backing, backing.format)));
        assert_eq!(alias_attachment.raw_format, 37);
        assert_eq!(alias_attachment.registered_format, 44);
        assert_eq!(alias_attachment.interpretation.unwrap().format, 44);
        assert_eq!(owner_sample.raw_format, 37);
        assert_eq!(owner_sample.interpretation.unwrap().format, 37);
        let at = point(55, 12, 18);
        assert!(alias_attachment.raw_filename(at, 0).ends_with("-fmt37.bin"));
        let mut s = Ledger::default();
        let observation = Observation {
            at,
            slot: 0,
            resource: alias_attachment,
            sample: Sample::Recorded,
        };
        s.producer(observation);
        assert_eq!(
            s.consumer(point(55, 13, 27), owner_sample),
            Link::Observation(observation)
        );

        // VW_FORMAT / shader storage override: one registered owner, different final view formats.
        let override_view = resolve(backing, 503, Some(view(503, backing, alias.format)));
        assert_eq!(override_view.registered_format, 37);
        assert_eq!(override_view.interpretation.unwrap().format, 44);
        assert_ne!(override_view.interpretation, owner_sample.interpretation);
        assert!(override_view.same_image(owner_sample));
        let same_format_alias = resolve(backing, 504, Some(view(504, backing, backing.format)));
        assert!(same_format_alias.same_image(owner_sample));

        // Actual storage fallback is captured AFTER selection, from the final create input.
        let srgb = image(vk::Format::R8G8B8A8_SRGB);
        let storage =
            resolve_resource(7, 4, srgb, srgb, 601, Some(view(601, srgb, backing.format)));
        let sampled = resolve_resource(7, 4, srgb, srgb, 602, Some(view(602, srgb, srgb.format)));
        assert_eq!(storage.raw_format, 43);
        assert_eq!(storage.registered_format, 43);
        assert_eq!(storage.interpretation.unwrap().format, 37);
        assert!(storage.same_image(sampled));
        assert_eq!(storage.bytes(), Some(1440 * 810 * 4));

        // A handle/registered format is insufficient to guess either an override or a fallback.
        for unknown in [
            resolve(alias, 501, None),
            resolve_resource(7, 4, srgb, srgb, 601, None),
            resolve(
                backing,
                503,
                Some(view(503, backing, vk::Format::UNDEFINED.as_raw())),
            ),
            resolve(backing, 0, Some(view(0, backing, backing.format))),
            resolve_resource(
                7,
                4,
                ImageInfo {
                    image: 200,
                    ..backing
                },
                backing,
                502,
                Some(view(502, backing, backing.format)),
            ),
        ] {
            assert_eq!(unknown.image, 0);
            assert_eq!(s.reserve(unknown), None);
            assert_eq!(s.consumer(point(55, 13, 27), unknown), Link::Unknown);
        }

        let good = view(502, backing, backing.format);
        for actual in [
            ResolvedView {
                aspect: vk::ImageAspectFlags::DEPTH.as_raw(),
                ..good
            },
            ResolvedView {
                base_level: 1,
                ..good
            },
            ResolvedView { layers: 2, ..good },
        ] {
            assert_eq!(s.reserve(resolve(backing, 502, Some(actual))), None);
        }
        let depth = image(vk::Format::D32_SFLOAT);
        let depth = resolve_resource(
            7,
            4,
            depth,
            depth,
            701,
            Some(view(701, depth, depth.format)),
        );
        assert_eq!(depth.aspect, vk::ImageAspectFlags::DEPTH.as_raw());
        assert_eq!(depth.interpretation.unwrap().aspect, depth.aspect);
        assert_eq!(s.reserve(depth), Some(1440 * 810 * 4));
        let unsupported = ImageInfo {
            format: vk::Format::D24_UNORM_S8_UINT.as_raw(),
            ..backing
        };
        let unsupported = resolve_resource(7, 4, unsupported, unsupported, 502, Some(good));
        assert_eq!(s.reserve(unsupported), None);
    }

    #[test]
    fn attachment_to_consumer_uses_backing_not_view_or_pass_number_alone() {
        let mut s = Ledger::default();
        let r = resource();
        let at = point(55, 12, 18);
        assert!(s.admit(at));
        let observation = Observation {
            at,
            slot: 0,
            resource: r,
            sample: Sample::Recorded,
        };
        s.producer(observation);
        let consumer = point(55, 13, 27);
        let other_view = Resource { view: 102, ..r };
        assert_eq!(
            s.consumer(consumer, other_view),
            Link::Observation(observation)
        );
        assert_eq!(
            s.consumer(consumer, Resource { writes: 5, ..r }),
            Link::ChangedSince(observation)
        );
        for changed in [
            Resource { image: 200, ..r },
            Resource { owner: 8, ..r },
            Resource { aspect: 2, ..r },
            Resource { width: 2880, ..r },
            Resource { image: 0, ..r },
        ] {
            assert_eq!(s.consumer(consumer, changed), Link::Unknown);
        }
        assert_eq!(s.consumer(point(56, 13, 27), r), Link::Unknown);
        let failed = Observation {
            at: point(55, 14, 29),
            sample: Sample::AllocationFailed,
            ..observation
        };
        s.producer(failed);
        assert_eq!(s.consumer(point(55, 15, 30), r), Link::Observation(failed));
        let unresolved = Resource {
            image: 0,
            bpp: 0,
            ..r
        };
        s.producer(Observation {
            resource: unresolved,
            ..failed
        });
        assert_eq!(s.consumer(point(55, 15, 30), unresolved), Link::Unknown);
        assert_eq!(s.reserve(unresolved), None);
    }

    #[test]
    fn metadata_and_readback_limits_cannot_be_reset_by_another_frame() {
        let mut s = Ledger::default();
        let at = point(55, 1, 0);
        for _ in 0..EVENTS {
            assert!(s.admit(at));
        }
        assert!(!s.admit(at));
        assert!(!s.admit(point(56, 2, 0)));
        let large = Resource {
            width: 4096,
            height: 4096,
            ..resource()
        };
        assert_eq!(s.reserve(large), Some(BYTES));
        assert_eq!(s.reserve(resource()), None);
        assert_eq!(s.bytes, BYTES);
        let mut s = Ledger::default();
        let small = Resource {
            width: 1,
            height: 1,
            ..resource()
        };
        for _ in 0..SAMPLES {
            assert_eq!(s.reserve(small), Some(4));
        }
        assert_eq!(s.reserve(small), None);
        assert_eq!(
            s.reserve(Resource {
                width: u32::MAX,
                height: u32::MAX,
                bpp: u32::MAX,
                ..small
            }),
            None
        );
        assert_eq!(s.reserve(Resource { width: 0, ..small }), None);
        assert_eq!(s.bytes, SAMPLES as u64 * 4);
    }

    #[test]
    fn evicted_attachment_is_unknown_and_retained_failure_is_not_success() {
        let mut s = Ledger::default();
        let base = resource();
        for n in 0..=RECORDS {
            s.producer(Observation {
                at: point(55, 1, n as u64),
                slot: 0,
                resource: Resource {
                    image: n as u64 + 1,
                    ..base
                },
                sample: Sample::BudgetOrExtent,
            });
        }
        assert_eq!(s.observations.len(), RECORDS);
        assert_eq!(s.evicted, 1);
        assert_eq!(
            s.consumer(point(55, 2, 300), Resource { image: 1, ..base }),
            Link::Unknown
        );
        assert!(matches!(
            s.consumer(point(55, 2, 300), Resource { image: 2, ..base }),
            Link::Observation(Observation {
                sample: Sample::BudgetOrExtent,
                ..
            })
        ));
    }
}

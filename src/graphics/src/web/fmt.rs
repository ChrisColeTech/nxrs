//! Vulkan formats as WebGPU texture and vertex formats, and the format features the web device reports for them.

use ash::vk;
use ash::vk::Format as F;

/// A texture format: WebGPU's name, bytes per block, block width and height, what it can be.
#[derive(Clone, Copy, Debug)]
pub struct Tex {
    pub name: &'static str,
    pub bytes: u32,
    pub bw: u32,
    pub bh: u32,
    pub filter: bool,
    pub render: bool,
    pub blend: bool,
    pub storage: bool,
    pub depth: bool,
    pub stencil: bool,
    /// "float", "unfilterable-float", "sint", "uint", "depth"
    pub sample: &'static str,
}

const fn c(name: &'static str, bytes: u32, sample: &'static str, render: bool, storage: bool) -> Tex {
    let float = matches!(sample.as_bytes()[0], b'f');
    Tex {
        name,
        bytes,
        bw: 1,
        bh: 1,
        filter: float,
        render,
        blend: render && float,
        storage,
        depth: false,
        stencil: false,
        sample,
    }
}
const fn bc(name: &'static str, bytes: u32) -> Tex {
    Tex {
        name,
        bytes,
        bw: 4,
        bh: 4,
        filter: true,
        render: false,
        blend: false,
        storage: false,
        depth: false,
        stencil: false,
        sample: "float",
    }
}
const fn ds(name: &'static str, bytes: u32, depth: bool, stencil: bool) -> Tex {
    Tex {
        name,
        bytes,
        bw: 1,
        bh: 1,
        filter: false,
        render: true,
        blend: false,
        storage: false,
        depth,
        stencil,
        sample: if depth { "depth" } else { "uint" },
    }
}

/// The WebGPU texture format of a Vulkan format on this device (its optional features applied), None when WebGPU has
/// none (the device reports no features for it).
pub fn tex(f: vk::Format) -> Option<Tex> {
    let feats = super::ring::hdr(super::ring::H_FEATURES).load(std::sync::atomic::Ordering::Relaxed);
    let t = tex_base(f)?;
    if t.name == "rg11b10ufloat" && feats & super::ring::F_RG11B10_RENDER == 0 {
        return Some(Tex { render: false, blend: false, ..t });
    }
    if t.name.contains("16unorm") && !t.depth && feats & super::ring::F_TIER1 == 0 {
        return None;
    }
    Some(t)
}

fn tex_base(f: vk::Format) -> Option<Tex> {
    Some(match f {
        // texture-formats-tier1's 16-bit normalized formats sample unfiltered
        F::R16_UNORM => Tex { blend: true, ..c("r16unorm", 2, "unfilterable-float", true, false) },
        F::R16G16_UNORM => Tex { blend: true, ..c("rg16unorm", 4, "unfilterable-float", true, false) },
        F::R16G16B16A16_UNORM => Tex { blend: true, ..c("rgba16unorm", 8, "unfilterable-float", true, false) },
        F::R8_UNORM => c("r8unorm", 1, "float", true, false),
        F::R8_SNORM => c("r8snorm", 1, "float", false, false),
        F::R8_UINT => c("r8uint", 1, "uint", true, false),
        F::R8_SINT => c("r8sint", 1, "sint", true, false),
        F::R16_UINT => c("r16uint", 2, "uint", true, false),
        F::R16_SINT => c("r16sint", 2, "sint", true, false),
        F::R16_SFLOAT => c("r16float", 2, "float", true, false),
        F::R8G8_UNORM => c("rg8unorm", 2, "float", true, false),
        F::R8G8_SNORM => c("rg8snorm", 2, "float", false, false),
        F::R8G8_UINT => c("rg8uint", 2, "uint", true, false),
        F::R8G8_SINT => c("rg8sint", 2, "sint", true, false),
        F::R32_UINT => c("r32uint", 4, "uint", true, true),
        F::R32_SINT => c("r32sint", 4, "sint", true, true),
        F::R32_SFLOAT => Tex { filter: false, blend: false, ..c("r32float", 4, "unfilterable-float", true, true) },
        F::R16G16_UINT => c("rg16uint", 4, "uint", true, false),
        F::R16G16_SINT => c("rg16sint", 4, "sint", true, false),
        F::R16G16_SFLOAT => c("rg16float", 4, "float", true, false),
        F::R8G8B8A8_UNORM | F::A8B8G8R8_UNORM_PACK32 => c("rgba8unorm", 4, "float", true, true),
        F::R8G8B8A8_SRGB | F::A8B8G8R8_SRGB_PACK32 => c("rgba8unorm-srgb", 4, "float", true, false),
        F::R8G8B8A8_SNORM | F::A8B8G8R8_SNORM_PACK32 => c("rgba8snorm", 4, "float", false, true),
        F::R8G8B8A8_UINT | F::A8B8G8R8_UINT_PACK32 => c("rgba8uint", 4, "uint", true, true),
        F::R8G8B8A8_SINT | F::A8B8G8R8_SINT_PACK32 => c("rgba8sint", 4, "sint", true, true),
        F::B8G8R8A8_UNORM => c("bgra8unorm", 4, "float", true, false),
        F::B8G8R8A8_SRGB => c("bgra8unorm-srgb", 4, "float", true, false),
        F::A2B10G10R10_UNORM_PACK32 => c("rgb10a2unorm", 4, "float", true, false),
        F::A2B10G10R10_UINT_PACK32 => c("rgb10a2uint", 4, "uint", true, false),
        F::B10G11R11_UFLOAT_PACK32 => c("rg11b10ufloat", 4, "float", true, false),
        F::E5B9G9R9_UFLOAT_PACK32 => c("rgb9e5ufloat", 4, "float", false, false),
        F::R32G32_UINT => c("rg32uint", 8, "uint", true, true),
        F::R32G32_SINT => c("rg32sint", 8, "sint", true, true),
        F::R32G32_SFLOAT => Tex { filter: false, blend: false, ..c("rg32float", 8, "unfilterable-float", true, true) },
        F::R16G16B16A16_UINT => c("rgba16uint", 8, "uint", true, true),
        F::R16G16B16A16_SINT => c("rgba16sint", 8, "sint", true, true),
        F::R16G16B16A16_SFLOAT => c("rgba16float", 8, "float", true, true),
        F::R32G32B32A32_UINT => c("rgba32uint", 16, "uint", true, true),
        F::R32G32B32A32_SINT => c("rgba32sint", 16, "sint", true, true),
        F::R32G32B32A32_SFLOAT => {
            Tex { filter: false, blend: false, ..c("rgba32float", 16, "unfilterable-float", true, true) }
        }
        F::D16_UNORM => ds("depth16unorm", 2, true, false),
        F::D32_SFLOAT => ds("depth32float", 4, true, false),
        F::X8_D24_UNORM_PACK32 => ds("depth24plus", 4, true, false),
        F::D32_SFLOAT_S8_UINT => ds("depth32float-stencil8", 8, true, true),
        F::S8_UINT => ds("stencil8", 1, false, true),
        F::BC1_RGBA_UNORM_BLOCK | F::BC1_RGB_UNORM_BLOCK => bc("bc1-rgba-unorm", 8),
        F::BC1_RGBA_SRGB_BLOCK | F::BC1_RGB_SRGB_BLOCK => bc("bc1-rgba-unorm-srgb", 8),
        F::BC2_UNORM_BLOCK => bc("bc2-rgba-unorm", 16),
        F::BC2_SRGB_BLOCK => bc("bc2-rgba-unorm-srgb", 16),
        F::BC3_UNORM_BLOCK => bc("bc3-rgba-unorm", 16),
        F::BC3_SRGB_BLOCK => bc("bc3-rgba-unorm-srgb", 16),
        F::BC4_UNORM_BLOCK => bc("bc4-r-unorm", 8),
        F::BC4_SNORM_BLOCK => bc("bc4-r-snorm", 8),
        F::BC5_UNORM_BLOCK => bc("bc5-rg-unorm", 16),
        F::BC5_SNORM_BLOCK => bc("bc5-rg-snorm", 16),
        F::BC6H_UFLOAT_BLOCK => bc("bc6h-rgb-ufloat", 16),
        F::BC6H_SFLOAT_BLOCK => bc("bc6h-rgb-float", 16),
        F::BC7_UNORM_BLOCK => bc("bc7-rgba-unorm", 16),
        F::BC7_SRGB_BLOCK => bc("bc7-rgba-unorm-srgb", 16),
        _ => return None,
    })
}

/// WebGPU's vertex format of a Vulkan format (None: not a WebGPU vertex format).
pub fn vertex(f: vk::Format) -> Option<&'static str> {
    Some(match f {
        F::R8_UINT => "uint8",
        F::R8_SINT => "sint8",
        F::R8_UNORM => "unorm8",
        F::R8_SNORM => "snorm8",
        F::R8G8_UINT => "uint8x2",
        F::R8G8_SINT => "sint8x2",
        F::R8G8_UNORM => "unorm8x2",
        F::R8G8_SNORM => "snorm8x2",
        F::R8G8B8A8_UINT => "uint8x4",
        F::R8G8B8A8_SINT => "sint8x4",
        F::R8G8B8A8_UNORM => "unorm8x4",
        F::R8G8B8A8_SNORM => "snorm8x4",
        F::B8G8R8A8_UNORM => "unorm8x4-bgra",
        F::R16_UINT => "uint16",
        F::R16_SINT => "sint16",
        F::R16_UNORM => "unorm16",
        F::R16_SNORM => "snorm16",
        F::R16_SFLOAT => "float16",
        F::R16G16_UINT => "uint16x2",
        F::R16G16_SINT => "sint16x2",
        F::R16G16_UNORM => "unorm16x2",
        F::R16G16_SNORM => "snorm16x2",
        F::R16G16_SFLOAT => "float16x2",
        F::R16G16B16A16_UINT => "uint16x4",
        F::R16G16B16A16_SINT => "sint16x4",
        F::R16G16B16A16_UNORM => "unorm16x4",
        F::R16G16B16A16_SNORM => "snorm16x4",
        F::R16G16B16A16_SFLOAT => "float16x4",
        F::R32_SFLOAT => "float32",
        F::R32G32_SFLOAT => "float32x2",
        F::R32G32B32_SFLOAT => "float32x3",
        F::R32G32B32A32_SFLOAT => "float32x4",
        F::R32_UINT => "uint32",
        F::R32G32_UINT => "uint32x2",
        F::R32G32B32_UINT => "uint32x3",
        F::R32G32B32A32_UINT => "uint32x4",
        F::R32_SINT => "sint32",
        F::R32G32_SINT => "sint32x2",
        F::R32G32B32_SINT => "sint32x3",
        F::R32G32B32A32_SINT => "sint32x4",
        F::A2B10G10R10_UNORM_PACK32 => "unorm10-10-10-2",
        _ => return None,
    })
}

/// The format features of `f` on the web device (optimal tiling; linear the same; buffer features: vertex only).
pub fn features(f: vk::Format) -> vk::FormatProperties {
    use vk::FormatFeatureFlags as Ff;
    let mut opt = Ff::empty();
    if let Some(t) = tex(f) {
        opt |= Ff::SAMPLED_IMAGE | Ff::TRANSFER_SRC | Ff::TRANSFER_DST | Ff::BLIT_SRC;
        if t.filter {
            opt |= Ff::SAMPLED_IMAGE_FILTER_LINEAR;
        }
        if t.render {
            if t.depth || t.stencil {
                opt |= Ff::DEPTH_STENCIL_ATTACHMENT;
            } else {
                opt |= Ff::COLOR_ATTACHMENT | Ff::BLIT_DST;
            }
        }
        if t.blend {
            opt |= Ff::COLOR_ATTACHMENT_BLEND;
        }
        if t.storage {
            opt |= Ff::STORAGE_IMAGE;
        }
    }
    let buf = if vertex(f).is_some() { Ff::VERTEX_BUFFER } else { Ff::empty() };
    vk::FormatProperties { linear_tiling_features: opt, optimal_tiling_features: opt, buffer_features: buf }
}

/// The aspect bits of a format.
pub fn aspects(f: vk::Format) -> vk::ImageAspectFlags {
    match tex(f) {
        Some(t) if t.depth && t.stencil => vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
        Some(t) if t.depth => vk::ImageAspectFlags::DEPTH,
        Some(t) if t.stencil => vk::ImageAspectFlags::STENCIL,
        _ => vk::ImageAspectFlags::COLOR,
    }
}

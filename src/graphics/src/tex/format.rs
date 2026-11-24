//! NVN formats as Vulkan formats: textures (with the swizzle a format without its own Vulkan layout needs) and vertex
//! attributes.

use ash::vk;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Float,
    Uint,
    Sint,
    Depth,
    DepthStencil,
    Stencil,
}

#[derive(Clone, Copy, Debug)]
pub struct TexFormat {
    pub vk: vk::Format,
    pub kind: Kind,
    /// RGB without alpha shown with alpha 1 (RGBX formats)
    pub alpha_one: bool,
    /// 3-component formats Vulkan cannot sample: stored as 4 components, expanded on upload (bytes per component)
    pub expand3: u32,
    /// ASTC block footprint and sRGB, decoded on the GPU
    pub astc: Option<(u32, u32, bool)>,
    pub srgb: bool,
}

const fn t(vk: vk::Format, kind: Kind) -> TexFormat {
    TexFormat {
        vk,
        kind,
        alpha_one: false,
        expand3: 0,
        astc: None,
        srgb: false,
    }
}
const fn x(vk: vk::Format, kind: Kind) -> TexFormat {
    TexFormat {
        vk,
        kind,
        alpha_one: true,
        expand3: 0,
        astc: None,
        srgb: false,
    }
}
const fn e3(vk: vk::Format, kind: Kind, comp: u32) -> TexFormat {
    TexFormat {
        vk,
        kind,
        alpha_one: true,
        expand3: comp,
        astc: None,
        srgb: false,
    }
}
const fn s(vk: vk::Format) -> TexFormat {
    TexFormat {
        vk,
        kind: Kind::Float,
        alpha_one: false,
        expand3: 0,
        astc: None,
        srgb: true,
    }
}

/// The texture format of an NVN format; None for an unknown value.
pub fn tex_format(f: i32) -> Option<TexFormat> {
    use vk::Format as F;
    use Kind::*;
    if (0x79..=0x94).contains(&f) {
        const ASTC: [[u32; 2]; 14] = [
            [4, 4],
            [5, 4],
            [5, 5],
            [6, 5],
            [6, 6],
            [8, 5],
            [8, 6],
            [8, 8],
            [10, 5],
            [10, 6],
            [10, 8],
            [10, 10],
            [12, 10],
            [12, 12],
        ];
        let k = ((f - 0x79) % 14) as usize;
        let srgb = f >= 0x87;
        return Some(TexFormat {
            vk: if srgb {
                F::R8G8B8A8_SRGB
            } else {
                F::R8G8B8A8_UNORM
            },
            kind: Float,
            alpha_one: false,
            expand3: 0,
            astc: Some((ASTC[k][0], ASTC[k][1], srgb)),
            srgb,
        });
    }
    Some(match f {
        1 => t(F::R8_UNORM, Float),
        2 => t(F::R8_SNORM, Float),
        3 => t(F::R8_UINT, Uint),
        4 => t(F::R8_SINT, Sint),
        5 => t(F::R16_SFLOAT, Float),
        6 => t(F::R16_UNORM, Float),
        7 => t(F::R16_SNORM, Float),
        8 => t(F::R16_UINT, Uint),
        9 => t(F::R16_SINT, Sint),
        10 => t(F::R32_SFLOAT, Float),
        11 => t(F::R32_UINT, Uint),
        12 => t(F::R32_SINT, Sint),
        13 => t(F::R8G8_UNORM, Float),
        14 => t(F::R8G8_SNORM, Float),
        15 => t(F::R8G8_UINT, Uint),
        16 => t(F::R8G8_SINT, Sint),
        17 => t(F::R16G16_SFLOAT, Float),
        18 => t(F::R16G16_UNORM, Float),
        19 => t(F::R16G16_SNORM, Float),
        20 => t(F::R16G16_UINT, Uint),
        21 => t(F::R16G16_SINT, Sint),
        22 => t(F::R32G32_SFLOAT, Float),
        23 => t(F::R32G32_UINT, Uint),
        24 => t(F::R32G32_SINT, Sint),
        25 => e3(F::R8G8B8A8_UNORM, Float, 1),
        26 => e3(F::R8G8B8A8_SNORM, Float, 1),
        27 => e3(F::R8G8B8A8_UINT, Uint, 1),
        28 => e3(F::R8G8B8A8_SINT, Sint, 1),
        29 => e3(F::R16G16B16A16_SFLOAT, Float, 2),
        30 => e3(F::R16G16B16A16_UNORM, Float, 2),
        31 => e3(F::R16G16B16A16_SNORM, Float, 2),
        32 => e3(F::R16G16B16A16_UINT, Uint, 2),
        33 => e3(F::R16G16B16A16_SINT, Sint, 2),
        34 => e3(F::R32G32B32A32_SFLOAT, Float, 4),
        35 => e3(F::R32G32B32A32_UINT, Uint, 4),
        36 => e3(F::R32G32B32A32_SINT, Sint, 4),
        37 => t(F::R8G8B8A8_UNORM, Float),
        38 => t(F::R8G8B8A8_SNORM, Float),
        39 => t(F::R8G8B8A8_UINT, Uint),
        40 => t(F::R8G8B8A8_SINT, Sint),
        41 => t(F::R16G16B16A16_SFLOAT, Float),
        42 => t(F::R16G16B16A16_UNORM, Float),
        43 => t(F::R16G16B16A16_SNORM, Float),
        44 => t(F::R16G16B16A16_UINT, Uint),
        45 => t(F::R16G16B16A16_SINT, Sint),
        46 => t(F::R32G32B32A32_SFLOAT, Float),
        47 => t(F::R32G32B32A32_UINT, Uint),
        48 => t(F::R32G32B32A32_SINT, Sint),
        49 => t(F::S8_UINT, Stencil),
        50 => t(F::D16_UNORM, Depth),
        51 => t(F::X8_D24_UNORM_PACK32, Depth),
        52 => t(F::D32_SFLOAT, Depth),
        53 => t(F::D24_UNORM_S8_UINT, DepthStencil),
        54 => t(F::D32_SFLOAT_S8_UINT, DepthStencil),
        55 => TexFormat {
            alpha_one: true,
            ..s(F::R8G8B8A8_SRGB)
        },
        56 => s(F::R8G8B8A8_SRGB),
        57 => t(F::A4B4G4R4_UNORM_PACK16, Float),
        58 => x(F::A1R5G5B5_UNORM_PACK16, Float),
        59 => t(F::A1R5G5B5_UNORM_PACK16, Float),
        60 => t(F::B5G6R5_UNORM_PACK16, Float),
        61 => t(F::A2B10G10R10_UNORM_PACK32, Float),
        62 => t(F::A2B10G10R10_UINT_PACK32, Uint),
        63 => t(F::B10G11R11_UFLOAT_PACK32, Float),
        64 => t(F::E5B9G9R9_UFLOAT_PACK32, Float),
        65 => x(F::BC1_RGB_UNORM_BLOCK, Float),
        66 => t(F::BC1_RGBA_UNORM_BLOCK, Float),
        67 => t(F::BC2_UNORM_BLOCK, Float),
        68 => t(F::BC3_UNORM_BLOCK, Float),
        69 => TexFormat {
            alpha_one: true,
            ..s(F::BC1_RGB_SRGB_BLOCK)
        },
        70 => s(F::BC1_RGBA_SRGB_BLOCK),
        71 => s(F::BC2_SRGB_BLOCK),
        72 => s(F::BC3_SRGB_BLOCK),
        73 => t(F::BC4_UNORM_BLOCK, Float),
        74 => t(F::BC4_SNORM_BLOCK, Float),
        75 => t(F::BC5_UNORM_BLOCK, Float),
        76 => t(F::BC5_SNORM_BLOCK, Float),
        77 => t(F::BC7_UNORM_BLOCK, Float),
        78 => s(F::BC7_SRGB_BLOCK),
        79 => t(F::BC6H_SFLOAT_BLOCK, Float),
        80 => t(F::BC6H_UFLOAT_BLOCK, Float),
        81 => t(F::R8_UINT, Uint),
        82 => t(F::R8_SINT, Sint),
        83 => t(F::R16_UINT, Uint),
        84 => t(F::R16_SINT, Sint),
        85 => t(F::R32_UINT, Uint),
        86 => t(F::R32_SINT, Sint),
        87 => t(F::R8G8_UINT, Uint),
        88 => t(F::R8G8_SINT, Sint),
        89 => t(F::R16G16_UINT, Uint),
        90 => t(F::R16G16_SINT, Sint),
        91 => t(F::R32G32_UINT, Uint),
        92 => t(F::R32G32_SINT, Sint),
        93 => e3(F::R8G8B8A8_UINT, Uint, 1),
        94 => e3(F::R8G8B8A8_SINT, Sint, 1),
        95 => e3(F::R16G16B16A16_UINT, Uint, 2),
        96 => e3(F::R16G16B16A16_SINT, Sint, 2),
        97 => e3(F::R32G32B32A32_UINT, Uint, 4),
        98 => e3(F::R32G32B32A32_SINT, Sint, 4),
        99 => t(F::R8G8B8A8_UINT, Uint),
        100 => t(F::R8G8B8A8_SINT, Sint),
        101 => t(F::R16G16B16A16_UINT, Uint),
        102 => t(F::R16G16B16A16_SINT, Sint),
        103 => t(F::R32G32B32A32_UINT, Uint),
        104 => t(F::R32G32B32A32_SINT, Sint),
        105 => t(F::A2B10G10R10_SNORM_PACK32, Float),
        106 => t(F::A2B10G10R10_SINT_PACK32, Sint),
        107 => t(F::A2B10G10R10_UINT_PACK32, Uint),
        108 => t(F::A2B10G10R10_SINT_PACK32, Sint),
        109 => x(F::R8G8B8A8_UNORM, Float),
        110 => x(F::R8G8B8A8_SNORM, Float),
        111 => x(F::R8G8B8A8_UINT, Uint),
        112 => x(F::R8G8B8A8_SINT, Sint),
        113 => x(F::R16G16B16A16_SFLOAT, Float),
        114 => x(F::R16G16B16A16_UNORM, Float),
        115 => x(F::R16G16B16A16_SNORM, Float),
        116 => x(F::R16G16B16A16_UINT, Uint),
        117 => x(F::R16G16B16A16_SINT, Sint),
        118 => x(F::R32G32B32A32_SFLOAT, Float),
        119 => x(F::R32G32B32A32_UINT, Uint),
        120 => x(F::R32G32B32A32_SINT, Sint),
        149 => t(F::R5G6B5_UNORM_PACK16, Float),
        150 => x(F::A1R5G5B5_UNORM_PACK16, Float),
        151 => t(F::A1R5G5B5_UNORM_PACK16, Float),
        152 => t(F::R5G5B5A1_UNORM_PACK16, Float),
        153 => x(F::B8G8R8A8_UNORM, Float),
        154 => t(F::B8G8R8A8_UNORM, Float),
        155 => TexFormat {
            alpha_one: true,
            ..s(F::B8G8R8A8_SRGB)
        },
        156 => s(F::B8G8R8A8_SRGB),
        _ => return None,
    })
}

/// The Vulkan format of an NVN vertex attribute format.
pub fn vertex_format(f: i32) -> vk::Format {
    use vk::Format as F;
    match f {
        25 => F::R8G8B8_UNORM,
        26 => F::R8G8B8_SNORM,
        27 => F::R8G8B8_UINT,
        28 => F::R8G8B8_SINT,
        29 => F::R16G16B16_SFLOAT,
        30 => F::R16G16B16_UNORM,
        31 => F::R16G16B16_SNORM,
        32 => F::R16G16B16_UINT,
        33 => F::R16G16B16_SINT,
        34 => F::R32G32B32_SFLOAT,
        35 => F::R32G32B32_UINT,
        36 => F::R32G32B32_SINT,
        81 => F::R8_USCALED,
        82 => F::R8_SSCALED,
        83 => F::R16_USCALED,
        84 => F::R16_SSCALED,
        85 => F::R32_UINT,
        86 => F::R32_SINT,
        87 => F::R8G8_USCALED,
        88 => F::R8G8_SSCALED,
        89 => F::R16G16_USCALED,
        90 => F::R16G16_SSCALED,
        91 => F::R32G32_UINT,
        92 => F::R32G32_SINT,
        93 => F::R8G8B8_USCALED,
        94 => F::R8G8B8_SSCALED,
        95 => F::R16G16B16_USCALED,
        96 => F::R16G16B16_SSCALED,
        97 => F::R32G32B32_UINT,
        98 => F::R32G32B32_SINT,
        99 => F::R8G8B8A8_USCALED,
        100 => F::R8G8B8A8_SSCALED,
        101 => F::R16G16B16A16_USCALED,
        102 => F::R16G16B16A16_SSCALED,
        103 => F::R32G32B32A32_UINT,
        104 => F::R32G32B32A32_SINT,
        107 => F::A2B10G10R10_USCALED_PACK32,
        108 => F::A2B10G10R10_SSCALED_PACK32,
        _ => tex_format(f).map(|t| t.vk).unwrap_or(F::UNDEFINED),
    }
}

pub fn is_depth_stencil(f: i32) -> bool {
    (49..=54).contains(&f)
}

pub fn aspect_of(kind: Kind) -> vk::ImageAspectFlags {
    match kind {
        Kind::Depth => vk::ImageAspectFlags::DEPTH,
        Kind::DepthStencil => vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
        Kind::Stencil => vk::ImageAspectFlags::STENCIL,
        _ => vk::ImageAspectFlags::COLOR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_format_maps() {
        for f in 1..=156 {
            let tf = tex_format(f).unwrap_or_else(|| panic!("format {f}"));
            assert_ne!(tf.vk, vk::Format::UNDEFINED, "format {f}");
            assert_ne!(vertex_format(f), vk::Format::UNDEFINED, "vertex format {f}");
        }
        assert!(tex_format(0).is_none());
        assert_eq!(tex_format(0x79).unwrap().astc, Some((4, 4, false)));
        assert_eq!(tex_format(0x94).unwrap().astc, Some((12, 12, true)));
    }
}

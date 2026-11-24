//! NDVE model contract and validation gate. No dispatch may be consumed until both its
//! arithmetic and its guest tensor layout have been certified. In particular, the Metal
//! reference's skip-prefix/fail-later policy is not safe for Vulkan replay.

#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod gpu_tests;
pub mod kernels;
pub(crate) mod prepare;
pub(crate) mod private_image;
pub(crate) mod trace;
#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;

pub const KEYS: [u64; 14] = [
    0x49dff010b9832251,
    0xba079f68ae288dba,
    0xa8b4fb48fee0ccf8,
    0xe2eebd7ba18fe35f,
    0xe6be82d835fbe4d3,
    0x9442573ee4ace011,
    0x0a56002cdc7e1fce,
    0x0a56002cdc7e1fce,
    0xd15370a382630d4e,
    0xe6be82d835fbe4d3,
    0x7f7ff0f36b54d90f,
    0xa8b4fb48fee0ccf8,
    0x5a7d69b067a7f011,
    0x4f41ccf24f163e0d,
];
pub const WEIGHT_BYTES: u64 = 0x3c320;

pub const NETWORK_DISPATCHES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    Pack,
    Conv16,
    Conv32,
    Conv64,
    Down16,
    Down32,
    Up64,
    Up32,
    Out,
    Luma,
    Resample,
    Compare,
}

impl Kernel {
    pub fn shape(self) -> [u32; 3] {
        match self {
            Self::Conv16 | Self::Out => [9, 16, 16],
            Self::Conv32 => [9, 32, 32],
            Self::Conv64 => [9, 64, 64],
            Self::Down16 => [4, 16, 32],
            Self::Down32 => [4, 32, 64],
            Self::Up64 => [4, 64, 32],
            Self::Up32 => [4, 32, 16],
            _ => [0; 3],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub kernel: Kernel,
    pub input: usize,
    pub output: usize,
    pub skip: Option<usize>,
}
// NHWC tensors, one zero border. Index 8 denotes the picture, not a tensor.
pub const TENSORS: [[u32; 3]; 8] = [
    [960, 540, 16],
    [960, 540, 16],
    [960, 540, 16],
    [480, 270, 32],
    [480, 270, 32],
    [480, 270, 32],
    [240, 135, 64],
    [240, 135, 64],
];
pub const LAYERS: [Layer; 13] = [
    Layer {
        kernel: Kernel::Pack,
        input: 8,
        output: 0,
        skip: None,
    },
    Layer {
        kernel: Kernel::Conv16,
        input: 0,
        output: 1,
        skip: None,
    },
    Layer {
        kernel: Kernel::Conv16,
        input: 1,
        output: 2,
        skip: None,
    },
    Layer {
        kernel: Kernel::Down16,
        input: 2,
        output: 3,
        skip: None,
    },
    Layer {
        kernel: Kernel::Conv32,
        input: 3,
        output: 4,
        skip: None,
    },
    Layer {
        kernel: Kernel::Down32,
        input: 4,
        output: 6,
        skip: None,
    },
    Layer {
        kernel: Kernel::Conv64,
        input: 6,
        output: 7,
        skip: None,
    },
    Layer {
        kernel: Kernel::Conv64,
        input: 7,
        output: 6,
        skip: None,
    },
    Layer {
        kernel: Kernel::Up64,
        input: 6,
        output: 3,
        skip: Some(4),
    },
    Layer {
        kernel: Kernel::Conv32,
        input: 3,
        output: 5,
        skip: None,
    },
    Layer {
        kernel: Kernel::Up32,
        input: 5,
        output: 1,
        skip: Some(2),
    },
    Layer {
        kernel: Kernel::Conv16,
        input: 1,
        output: 0,
        skip: None,
    },
    Layer {
        kernel: Kernel::Out,
        input: 0,
        output: 8,
        skip: None,
    },
];

/// Weights are supplied by the caller. Only the model's layout is built into the host.
pub fn offsets() -> [(u64, u64); 12] {
    let mut o = 0;
    std::array::from_fn(|i| {
        let [t, k, n] = LAYERS[i + 1].kernel.shape();
        let w = o;
        o += (u64::from(t * k * n * 2) + 255) & !255;
        let b = o;
        o += if i == 11 { u64::from(n * 2) } else { 256 };
        (w, b)
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub address: u64,
    pub size: u64,
}
impl Span {
    pub fn contains(self, address: u64, size: u64) -> bool {
        self.address != 0
            && size != 0
            && address >= self.address
            && address.checked_add(size).is_some_and(|end| {
                self.address
                    .checked_add(self.size)
                    .is_some_and(|limit| end <= limit)
            })
    }
}

/// An observation is never permission to skip. Only a complete, immutable graph can
/// be validated; the recorder must additionally certify tensor bindings and residency.
#[derive(Clone, Copy, Debug)]
pub struct Dispatch {
    pub key: u64,
    pub weights: Span,
    pub bias: Span,
    pub groups: [u32; 3],
    pub indirect: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageBinding {
    pub id: i32,
    pub serial: u64,
    pub width: u32,
    pub height: u32,
    pub format: i32,
    pub image: u64,
    pub sampled_view: u64,
    pub storage_view: u64,
    pub usage: u32,
    pub generation: u64,
    pub guest: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameResources {
    pub picture: ImageBinding,
    pub network_output: ImageBinding,
    pub dither: ImageBinding,
    pub final_output: ImageBinding,
    pub resample_weights: Span,
    pub constants: [u8; 96],
}

impl FrameResources {
    pub fn validate(self, first_picture: i32) -> Result<(), &'static str> {
        use ash::vk;
        let transfer_src = vk::ImageUsageFlags::TRANSFER_SRC.as_raw();
        let transfer_dst = vk::ImageUsageFlags::TRANSFER_DST.as_raw();
        let image =
            |b: ImageBinding, width, height, format, usage, sampled: bool, storage: bool| {
                b.id > 0
                    && b.serial != 0
                    && b.generation != 0
                    && b.guest.contains(b.guest.address, b.guest.size)
                    && b.guest.size >= u64::from(width) * u64::from(height) * 4
                    && b.image != 0
                    && (!sampled || b.sampled_view != 0)
                    && (!storage || b.storage_view != 0)
                    && b.width == width
                    && b.height == height
                    && b.format == format
                    && b.usage & usage == usage
            };
        if self.picture.id != first_picture
            || !image(
                self.picture,
                1920,
                1080,
                vk::Format::R8G8B8A8_UNORM.as_raw(),
                transfer_src,
                true,
                false,
            )
            || !image(
                self.network_output,
                1920,
                1080,
                vk::Format::A2B10G10R10_UNORM_PACK32.as_raw(),
                transfer_dst | transfer_src,
                true,
                true,
            )
            || !image(
                self.dither,
                512,
                512,
                vk::Format::A2B10G10R10_UNORM_PACK32.as_raw(),
                transfer_src,
                true,
                false,
            )
            || !image(
                self.final_output,
                2560,
                1440,
                vk::Format::R8G8B8A8_SRGB.as_raw(),
                transfer_dst | transfer_src,
                false,
                true,
            )
        {
            return Err("image identity, format, dimensions, or transfer ownership");
        }
        let images = [self.picture, self.network_output, self.dither, self.final_output];
        if images.iter().enumerate().any(|(i, a)| images[i + 1..].iter().any(|b| {
            a.image == b.image || (a.guest.address < b.guest.address + b.guest.size
                && b.guest.address < a.guest.address + a.guest.size)
        })) {
            return Err("input/output image alias");
        }
        if self.resample_weights.size < 0x3000 || self.resample_weights.address == 0 {
            return Err("resampler weights");
        }
        let c = self.constants.map(|x| x);
        let i = |word: usize| i32::from_le_bytes(c[4 * word..4 * word + 4].try_into().unwrap());
        if [i(0), i(1), i(2), i(3), i(4), i(5), i(6), i(7), i(20)]
            != [0, 0, 1919, 1079, 0, 0, 2559, 1439, 1]
        {
            return Err("resampler constants");
        }
        Ok(())
    }
}

pub fn validate_sequence(ds: &[Dispatch], table: Span) -> Result<(), &'static str> {
    if ds.len() != 12 {
        return Err("incomplete network (no prefix may be skipped)");
    }
    if table.size != WEIGHT_BYTES || !table.contains(table.address, WEIGHT_BYTES) {
        return Err("weight table size or address");
    }
    for (i, (d, (w, b))) in ds.iter().zip(offsets()).enumerate() {
        let [t, k, n] = LAYERS[i + 1].kernel.shape();
        if d.key != KEYS[i + 1] {
            return Err("shader sequence");
        }
        if d.indirect || d.groups.contains(&0) {
            return Err("indirect or empty dispatch");
        }
        // Minimum extents, not merely non-null buffers as in the reference host.
        if d.weights.address != table.address + w
            || d.bias.address != table.address + b
            || !d
                .weights
                .contains(table.address + w, u64::from(t * k * n * 2))
            || !d.bias.contains(table.address + b, u64::from(n * 2))
        {
            return Err("weight/bias extent or offset");
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Check,
    On,
}

impl Mode {
    pub fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        // Check also ran a full native graph every frame and wrote guest outputs
        // before fallback. No live mode is certified under the incident hold.
        super::queue::submission_safety::ndve_mode_permission(value)?;
        Ok(Self::Off)
    }
}

pub(crate) fn mode() -> Mode {
    static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
    *MODE.get_or_init(
        || match Mode::parse(std::env::var("NXGFX_NDVE").ok().as_deref()) {
            Ok(mode) => mode,
            Err(e) => {
                crate::log::red(&format!("NDVE: {e}; all translated dispatches retained"));
                Mode::Off
            }
        },
    )
}

/// Report refused live modes before acquiring a translated NDVE pipeline.
/// The ordinary guest dispatch remains intact; this is not permission to run a game.
pub(crate) fn before_dispatch(key: u64) {
    if KEYS.contains(&key) {
        let _ = mode();
    }
}

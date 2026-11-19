//! NVN's public types as the game's SDK lays them out: opaque objects of the reserved sizes the game allocates, the
//! small plain structs passed by pointer, and the enum values the record layer and the replay read.
#![allow(non_camel_case_types, non_upper_case_globals, dead_code)]

macro_rules! opaque {
    ($($name:ident = $size:expr;)*) => {$(
        #[repr(C, align(8))]
        pub struct $name(pub [u8; $size]);
        impl $name {
            /// A zeroed object, boxed (the game's own allocation in tests and tools).
            pub fn boxed() -> Box<Self> { Box::new($name([0u8; $size])) }
        }
    )*};
}

opaque! {
    NVNdevice = 0x3000;
    NVNdeviceBuilder = 0x40;
    NVNmultisampleState = 0x18;
    NVNdepthStencilState = 0x8;
    NVNchannelMaskState = 0x4;
    NVNpolygonState = 0x4;
    NVNcolorState = 0x4;
    NVNqueue = 0x2000;
    NVNqueueBuilder = 0x40;
    NVNwindow = 384;
    NVNwindowBuilder = 0x40;
    NVNtexture = 0x80;
    NVNtextureView = 0x10;
    NVNtextureBuilder = 0x80;
    NVNcommandBuffer = 0xA0;
    NVNsync = 0x40;
    NVNprogram = 0xC0;
    NVNmemoryPool = 0x100;
    NVNmemoryPoolBuilder = 0x40;
    NVNtexturePool = 0x20;
    NVNsamplerPool = 0x20;
    NVNblendState = 0x8;
    NVNvertexAttribState = 0x4;
    NVNvertexStreamState = 0x8;
    NVNsamplerBuilder = 0x60;
    NVNsampler = 0x30;
    NVNbufferBuilder = 0x40;
    NVNbuffer = 0x30;
    NVNevent = 0x40;
    NVNeventBuilder = 0x20;
}

pub type NVNboolean = u8;
pub type NVNtextureHandle = u64;
pub type NVNimageHandle = u64;
pub type NVNcommandHandle = u64;
pub type NVNbufferAddress = u64;
pub type NVNtextureAddress = u64;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct NVNbufferRange {
    pub address: u64,
    pub size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct NVNcopyRegion {
    pub xoffset: i32,
    pub yoffset: i32,
    pub zoffset: i32,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NVNshaderData {
    pub data: u64,
    pub control: *const u8,
}
impl Default for NVNshaderData {
    fn default() -> Self {
        NVNshaderData { data: 0, control: core::ptr::null() }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NVNrectangle {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NVNcounterData {
    pub counter: u64,
    pub timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NVNpackagedTextureLayout {
    pub layout: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NVNseparateTextureHandle {
    pub value: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NVNseparateSamplerHandle {
    pub value: u64,
}

/// `void (*)(NVNcommandBuffer *, NVNcommandBufferMemoryEvent, size_t, void *)`: called on the game's code (a guest
/// function) when command or control memory runs out. The record layer calls it through `callbacks::call_guest`.
pub type PFNNVNCOMMANDBUFFERMEMORYCALLBACKPROC = u64;

// ---- enums (values of the SDK's nvn.h) ----
pub mod mem_flags {
    pub const CPU_NO_ACCESS: u32 = 1;
    pub const CPU_UNCACHED: u32 = 2;
    pub const CPU_CACHED: u32 = 4;
    pub const GPU_NO_ACCESS: u32 = 8;
    pub const GPU_UNCACHED: u32 = 16;
    pub const GPU_CACHED: u32 = 32;
    pub const SHADER_CODE: u32 = 64;
    pub const COMPRESSIBLE: u32 = 128;
    pub const PHYSICAL: u32 = 256;
    pub const VIRTUAL: u32 = 512;
}

pub mod tex_flags {
    pub const DISPLAY: u32 = 1;
    pub const VIDEO_DECODE: u32 = 2;
    pub const IMAGE: u32 = 4;
    pub const COMPRESSIBLE: u32 = 8;
    pub const LINEAR: u32 = 16;
    pub const SPARSE: u32 = 32;
    pub const LINEAR_RENDER_TARGET: u32 = 64;
    pub const MINIMAL_LAYOUT: u32 = 512;
}

pub mod target {
    pub const T1D: i32 = 0;
    pub const T2D: i32 = 1;
    pub const T3D: i32 = 2;
    pub const T1D_ARRAY: i32 = 3;
    pub const T2D_ARRAY: i32 = 4;
    pub const T2D_MS: i32 = 5;
    pub const T2D_MS_ARRAY: i32 = 6;
    pub const RECTANGLE: i32 = 7;
    pub const CUBE: i32 = 8;
    pub const CUBE_ARRAY: i32 = 9;
    pub const BUFFER: i32 = 10;
}

/// NVNshaderStage: also the record layer's stage numbering, and the descriptor set of a stage's resources.
pub mod stage {
    pub const VERTEX: u32 = 0;
    pub const FRAGMENT: u32 = 1;
    pub const GEOMETRY: u32 = 2;
    pub const TESS_CONTROL: u32 = 3;
    pub const TESS_EVALUATION: u32 = 4;
    pub const COMPUTE: u32 = 5;
}

pub mod prim {
    pub const POINTS: i32 = 0;
    pub const LINES: i32 = 1;
    pub const LINE_LOOP: i32 = 2;
    pub const LINE_STRIP: i32 = 3;
    pub const TRIANGLES: i32 = 4;
    pub const TRIANGLE_STRIP: i32 = 5;
    pub const TRIANGLE_FAN: i32 = 6;
    pub const QUADS: i32 = 7;
    pub const QUAD_STRIP: i32 = 8;
    pub const POLYGON: i32 = 9;
    pub const LINES_ADJACENCY: i32 = 10;
    pub const LINE_STRIP_ADJACENCY: i32 = 11;
    pub const TRIANGLES_ADJACENCY: i32 = 12;
    pub const TRIANGLE_STRIP_ADJACENCY: i32 = 13;
    pub const PATCHES: i32 = 14;
}

pub mod index_type {
    pub const U8: i32 = 0;
    pub const U16: i32 = 1;
    pub const U32: i32 = 2;
}

pub mod blend_func {
    pub const ZERO: u32 = 1;
    pub const ONE: u32 = 2;
    pub const SRC_COLOR: u32 = 3;
    pub const ONE_MINUS_SRC_COLOR: u32 = 4;
    pub const SRC_ALPHA: u32 = 5;
    pub const ONE_MINUS_SRC_ALPHA: u32 = 6;
    pub const DST_ALPHA: u32 = 7;
    pub const ONE_MINUS_DST_ALPHA: u32 = 8;
    pub const DST_COLOR: u32 = 9;
    pub const ONE_MINUS_DST_COLOR: u32 = 10;
    pub const SRC_ALPHA_SATURATE: u32 = 11;
    pub const SRC1_COLOR: u32 = 16;
    pub const ONE_MINUS_SRC1_COLOR: u32 = 17;
    pub const SRC1_ALPHA: u32 = 18;
    pub const ONE_MINUS_SRC1_ALPHA: u32 = 19;
    pub const CONSTANT_COLOR: u32 = 97;
    pub const ONE_MINUS_CONSTANT_COLOR: u32 = 98;
    pub const CONSTANT_ALPHA: u32 = 99;
    pub const ONE_MINUS_CONSTANT_ALPHA: u32 = 100;
}

pub mod blend_eq {
    pub const ADD: u32 = 1;
    pub const SUB: u32 = 2;
    pub const REVERSE_SUB: u32 = 3;
    pub const MIN: u32 = 4;
    pub const MAX: u32 = 5;
}

/// Depth, stencil, alpha and sampler compare functions share these values (NEVER = 1 .. ALWAYS = 8).
pub mod cmp {
    pub const NEVER: u32 = 1;
    pub const LESS: u32 = 2;
    pub const EQUAL: u32 = 3;
    pub const LEQUAL: u32 = 4;
    pub const GREATER: u32 = 5;
    pub const NOTEQUAL: u32 = 6;
    pub const GEQUAL: u32 = 7;
    pub const ALWAYS: u32 = 8;
}

pub mod stencil_op {
    pub const KEEP: u32 = 1;
    pub const ZERO: u32 = 2;
    pub const REPLACE: u32 = 3;
    pub const INCR: u32 = 4;
    pub const DECR: u32 = 5;
    pub const INVERT: u32 = 6;
    pub const INCR_WRAP: u32 = 7;
    pub const DECR_WRAP: u32 = 8;
}

pub mod face {
    pub const NONE: u32 = 0;
    pub const FRONT: u32 = 1;
    pub const BACK: u32 = 2;
    pub const FRONT_AND_BACK: u32 = 3;
}

pub mod front_face {
    pub const CW: u32 = 0;
    pub const CCW: u32 = 1;
}

pub mod polygon_mode {
    pub const POINT: u32 = 0;
    pub const LINE: u32 = 1;
    pub const FILL: u32 = 2;
}

pub mod min_filter {
    pub const NEAREST: u8 = 0;
    pub const LINEAR: u8 = 1;
    pub const NEAREST_MIPMAP_NEAREST: u8 = 2;
    pub const LINEAR_MIPMAP_NEAREST: u8 = 3;
    pub const NEAREST_MIPMAP_LINEAR: u8 = 4;
    pub const LINEAR_MIPMAP_LINEAR: u8 = 5;
}

pub mod wrap {
    pub const CLAMP: u8 = 0;
    pub const REPEAT: u8 = 1;
    pub const MIRROR_CLAMP: u8 = 2;
    pub const MIRROR_CLAMP_TO_EDGE: u8 = 3;
    pub const MIRROR_CLAMP_TO_BORDER: u8 = 4;
    pub const CLAMP_TO_BORDER: u8 = 5;
    pub const MIRRORED_REPEAT: u8 = 6;
    pub const CLAMP_TO_EDGE: u8 = 7;
}

pub mod logic_op {
    pub const COPY: u32 = 3;
}

pub mod sync_wait {
    pub const ALREADY_SIGNALED: u32 = 0;
    pub const CONDITION_SATISFIED: u32 = 1;
    pub const TIMEOUT_EXPIRED: u32 = 2;
    pub const FAILED: u32 = 3;
}

pub mod event_signal {
    pub const WRITE: u32 = 0;
    pub const ADD: u32 = 1;
}

pub mod event_wait {
    pub const EQUAL: u32 = 0;
    pub const GEQUAL_WRAP: u32 = 1;
}

pub mod counter {
    pub const TIMESTAMP: u32 = 0;
    pub const SAMPLES_PASSED: u32 = 1;
    pub const TIMESTAMP_TOP: u32 = 16;
}

pub mod barrier {
    pub const ORDER_PRIMITIVES: u32 = 1;
    pub const ORDER_FRAGMENTS: u32 = 2;
    pub const ORDER_FRAGMENTS_TILED: u32 = 4;
    pub const ORDER_INDIRECT_DATA: u32 = 8;
    pub const INVALIDATE_TEXTURE: u32 = 16;
    pub const INVALIDATE_SHADER: u32 = 32;
    pub const INVALIDATE_TEXTURE_DESCRIPTOR: u32 = 64;
    pub const INVALIDATE_ZCULL: u32 = 128;
}

pub mod origin {
    pub const LOWER_LEFT: u32 = 0;
    pub const UPPER_LEFT: u32 = 1;
}

pub mod cmdbuf_mem_event {
    pub const OUT_OF_COMMAND_MEMORY: u32 = 0;
    pub const OUT_OF_CONTROL_MEMORY: u32 = 1;
}

pub mod swizzle {
    pub const ZERO: u8 = 0;
    pub const ONE: u8 = 1;
    pub const R: u8 = 2;
    pub const G: u8 = 3;
    pub const B: u8 = 4;
    pub const A: u8 = 5;
}

/// NVNformat values the code names; the full table is `tex::format`.
pub mod fmt {
    pub const NONE: i32 = 0;
    pub const R8: i32 = 1;
    pub const R32F: i32 = 10;
    pub const R32UI: i32 = 11;
    pub const RG32F: i32 = 22;
    pub const RGB32F: i32 = 34;
    pub const RGBA8: i32 = 37;
    pub const RGBA16F: i32 = 41;
    pub const RGBA32F: i32 = 46;
    pub const STENCIL8: i32 = 49;
    pub const DEPTH16: i32 = 50;
    pub const DEPTH24: i32 = 51;
    pub const DEPTH32F: i32 = 52;
    pub const DEPTH24_STENCIL8: i32 = 53;
    pub const DEPTH32F_STENCIL8: i32 = 54;
    pub const RGBA8_SRGB: i32 = 56;
    pub const BGRA8: i32 = 154;
    pub const ASTC_4X4: i32 = 0x79;
    pub const ASTC_12X12_SRGB: i32 = 0x94;
}

opaque! {
    NVNprogramBuilder = 0x80;
    NVNdlssNetworkBuilder = 0xC0;
    NVNdlssNetwork = 0x218;
    NVNdlssParameters = 0xC8;
    NVNdeviceCommandBuffer = 0x30;
    NVNdeviceMessageBuffer = 0x28;
}

/// nvnTexture(Builder)GetSparseTileLayout's result.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct NVNtextureSparseTileLayout {
    pub num_tiled_levels: i32,
    pub tile_width: i32,
    pub tile_height: i32,
    pub tile_depth: i32,
    pub reserved: [u8; 16],
}

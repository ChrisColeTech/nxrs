//! The web device's objects: what each Vulkan handle stands for, and the GPU worker's id of its WebGPU object.

use super::fmt;
use super::ring::{self, Pk, Word};
use ash::vk;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Packet ops (`src/front/web/page/gpu-worker.js` has the same table).
pub mod op {
    pub const BUFFER: u32 = 1;
    pub const DESTROY: u32 = 2;
    pub const TEXTURE: u32 = 3;
    pub const VIEW: u32 = 4;
    pub const SAMPLER: u32 = 5;
    pub const SHADER: u32 = 6;
    pub const BGL: u32 = 7;
    pub const PLAYOUT: u32 = 8;
    pub const RPIPE: u32 = 9;
    pub const CPIPE: u32 = 10;
    pub const BINDGROUP: u32 = 11;
    pub const SUBMIT_BEGIN: u32 = 20;
    pub const UPLOAD: u32 = 21;
    pub const UPLOAD_TEX: u32 = 22;
    pub const PASS_BEGIN: u32 = 23;
    pub const PASS_END: u32 = 24;
    pub const SET_PIPELINE: u32 = 25;
    pub const SET_BIND_GROUP: u32 = 26;
    pub const SET_VB: u32 = 27;
    pub const SET_IB: u32 = 28;
    pub const VIEWPORT: u32 = 29;
    pub const SCISSOR: u32 = 30;
    pub const BLEND_CONST: u32 = 31;
    pub const STENCIL_REF: u32 = 32;
    pub const DRAW: u32 = 33;
    pub const DRAW_INDEXED: u32 = 34;
    pub const DRAW_INDIRECT: u32 = 35;
    pub const COMPUTE_BEGIN: u32 = 36;
    pub const COMPUTE_END: u32 = 37;
    pub const DISPATCH: u32 = 38;
    pub const DISPATCH_INDIRECT: u32 = 39;
    pub const COPY_B2B: u32 = 40;
    pub const COPY_B2T: u32 = 41;
    pub const COPY_T2B: u32 = 42;
    pub const COPY_T2T: u32 = 43;
    pub const CLEAR_BUFFER: u32 = 44;
    pub const BLIT: u32 = 45;
    pub const CLEAR_TEX: u32 = 46;
    pub const READBACK: u32 = 47;
    pub const SIGNAL: u32 = 48;
    pub const SUBMIT_END: u32 = 49;
    pub const PRESENT: u32 = 50;
    pub const CLEAR_RECT: u32 = 51;
    pub const FILL_BUFFER: u32 = 52;
}

/// The worker's object ids (1: the per-draw uniform ring buffer, 2..: everything else).
pub const UNIFORM_RING: u32 = 1;
pub const UNIFORM_RING_SIZE: u64 = 32 << 20;
static NEXT_ID: AtomicU32 = AtomicU32::new(16);
pub fn new_id() -> u32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x1000);
pub fn new_handle() -> u64 {
    NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

pub fn json(op: u32, v: &serde_json::Value) {
    ring::push(op, v.to_string().as_bytes());
}

pub fn destroy(id: u32) {
    if id != 0 {
        ring::push(op::DESTROY, &id.to_le_bytes());
    }
}

pub struct Memory {
    pub size: u64,
    /// host-visible memory: its bytes in linear memory (null: device-local)
    pub host: *mut u8,
}

pub struct Buffer {
    pub size: u64,
    pub usage: vk::BufferUsageFlags,
    pub id: u32,
    /// the host bytes it is bound to (null: device-local or not bound)
    pub host: *mut u8,
}

#[derive(Clone)]
pub struct Image {
    pub ty: vk::ImageType,
    pub format: vk::Format,
    pub extent: vk::Extent3D,
    pub mips: u32,
    pub layers: u32,
    pub samples: u32,
    pub usage: vk::ImageUsageFlags,
    pub flags: vk::ImageCreateFlags,
    pub id: u32,
    pub tex: Option<fmt::Tex>,
}

#[derive(Clone)]
pub struct View {
    pub image: u64,
    pub id: u32,
    pub ty: vk::ImageViewType,
    pub format: vk::Format,
    pub base_mip: u32,
    pub mips: u32,
    pub base_layer: u32,
    pub layers: u32,
    pub aspect: vk::ImageAspectFlags,
    /// the worker view's swizzle (a narrower view of the same, WEB-W10c, keeps it)
    pub swizzle: String,
}

pub struct ShaderModule {
    pub conv: Result<Arc<shaders::wgsl::Wgsl>, String>,
    pub id: u32,
}

#[derive(Clone)]
pub struct Dsl {
    /// binding -> (type, count)
    pub bindings: HashMap<u32, (vk::DescriptorType, u32)>,
}

#[derive(Clone)]
pub struct PipelineLayout {
    pub sets: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Desc {
    None,
    Buf { buf: u64, off: u64, range: u64 },
    Img { view: u64, sampler: u64 },
    Sampler(u64),
    TexelBuf(u64),
}

pub struct DSet {
    pub pool: u64,
    /// (binding, array element) -> descriptor
    pub d: HashMap<(u32, u32), Desc>,
}

/// Graphics pipeline state taken at creation; what is dynamic comes from the command buffer at each draw.
/// A pipeline's stage: its WGSL facts and the worker's module id (kept: the module handle may be destroyed).
pub type Stage = (Arc<shaders::wgsl::Wgsl>, u32);

#[derive(Clone)]
pub struct Gfx {
    pub vs: Stage,
    pub fs: Option<Stage>,
    pub colors: Vec<vk::Format>,
    pub depth: vk::Format,
    pub stencil: vk::Format,
    pub dynamic: Vec<vk::DynamicState>,
    pub stat: super::cmd::Dyn,
}

pub enum Pipeline {
    Gfx(Box<Gfx>),
    Compute { cs: Stage },
}

/// A semaphore or fence: `word` is the value the GPU finished (written by the worker once the submission's work and
/// read-backs are done); `queued` the value of the last submission signalling it that the worker has handed to the
/// GPU (its CPU-side execute done), so a GPU-time budget starts there (`icd::wait_queued`).
pub struct Semaphore {
    pub word: Word,
    pub queued: Word,
}

impl Semaphore {
    pub fn new(v: u64) -> Semaphore {
        Semaphore { word: Word::new(v), queued: Word::new(v) }
    }
}

pub struct Swapchain {
    pub images: Vec<u64>,
    pub extent: vk::Extent2D,
    pub next: u32,
}

pub struct QueryPool {
    pub ty: vk::QueryType,
    pub count: u32,
}

#[derive(Default)]
pub struct Objs {
    pub mem: HashMap<u64, Memory>,
    pub buf: HashMap<u64, Buffer>,
    pub img: HashMap<u64, Image>,
    pub view: HashMap<u64, View>,
    pub sampler: HashMap<u64, u32>,
    pub module: HashMap<u64, ShaderModule>,
    pub dsl: HashMap<u64, Dsl>,
    pub playout: HashMap<u64, PipelineLayout>,
    pub dset: HashMap<u64, DSet>,
    pub pipe: HashMap<u64, Arc<Pipeline>>,
    pub sem: HashMap<u64, Semaphore>,
    pub swapchain: HashMap<u64, Swapchain>,
    pub query: HashMap<u64, QueryPool>,
}

unsafe impl Send for Objs {}

pub fn objs() -> std::sync::MutexGuard<'static, Objs> {
    static O: OnceLock<Mutex<Objs>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(Objs::default())).lock().unwrap()
}

// ---------- GPU object creation ----------

pub fn gpu_buffer(size: u64, usage: vk::BufferUsageFlags) -> u32 {
    use vk::BufferUsageFlags as B;
    // WebGPU: MAP_READ 1, MAP_WRITE 2, COPY_SRC 4, COPY_DST 8, INDEX 16, VERTEX 32, UNIFORM 64, STORAGE 128, INDIRECT 256
    // index and vertex use always: the replay binds index data it converted into buffers made for other uses
    let mut u = 4 | 8 | 16 | 32;
    if usage.intersects(B::UNIFORM_BUFFER | B::UNIFORM_TEXEL_BUFFER) {
        u |= 64;
    }
    if usage.intersects(B::STORAGE_BUFFER | B::STORAGE_TEXEL_BUFFER) {
        u |= 128;
    }
    if usage.contains(B::INDIRECT_BUFFER) {
        u |= 256;
    }
    let id = new_id();
    let mut p = Pk::default();
    p.u32(id).u32(u).u64(size.next_multiple_of(4));
    ring::push(op::BUFFER, &p.0);
    id
}

pub fn gpu_texture(im: &Image, canvas: bool) -> u32 {
    let Some(t) = im.tex else { return 0 };
    use vk::ImageUsageFlags as U;
    // WebGPU: COPY_SRC 1, COPY_DST 2, TEXTURE_BINDING 4, STORAGE_BINDING 8, RENDER_ATTACHMENT 16
    let mut u = 1 | 2;
    if t.name.starts_with("bc") {
        u |= 4;
    } else {
        if im.usage.contains(U::SAMPLED) || t.render {
            u |= 4;
        }
        if im.usage.contains(U::STORAGE) && t.storage && im.samples == 1 {
            u |= 8;
        }
        if t.render {
            u |= 16;
        }
    }
    if im.samples > 1 {
        // multisampled textures: render attachment and sampling only
        u &= 4 | 16;
    }
    let dim = match im.ty {
        vk::ImageType::TYPE_3D => "3d",
        vk::ImageType::TYPE_1D if im.mips == 1 && !t.render => "1d",
        _ => "2d",
    };
    let (w, h, d) = (
        im.extent.width.max(1),
        if dim == "1d" { 1 } else { im.extent.height.max(1) },
        if dim == "3d" { im.extent.depth.max(1) } else { im.layers.max(1) },
    );
    // the srgb / linear twin is a legal view format (MUTABLE_FORMAT views of other formats are not in WebGPU)
    let twin = if im.flags.contains(vk::ImageCreateFlags::MUTABLE_FORMAT) {
        t.name.strip_suffix("-srgb").map(String::from).or_else(|| {
            matches!(t.name, "rgba8unorm" | "bgra8unorm" | "bc1-rgba-unorm" | "bc2-rgba-unorm" | "bc3-rgba-unorm" | "bc7-rgba-unorm")
                .then(|| format!("{}-srgb", t.name))
        })
    } else {
        None
    };
    let id = new_id();
    json(
        op::TEXTURE,
        &serde_json::json!({
            "id": id, "format": t.name, "dim": dim, "w": w, "h": h, "d": d,
            "mips": if im.samples > 1 { 1 } else { im.mips.max(1) },
            "samples": if im.samples > 1 { 4 } else { 1 }, "usage": u,
            "viewFormats": twin.map(|f| vec![f]).unwrap_or_default(), "canvas": canvas,
        }),
    );
    id
}

pub fn view_dim(t: vk::ImageViewType) -> &'static str {
    match t {
        vk::ImageViewType::TYPE_1D => "1d",
        vk::ImageViewType::TYPE_3D => "3d",
        vk::ImageViewType::CUBE => "cube",
        vk::ImageViewType::CUBE_ARRAY => "cube-array",
        vk::ImageViewType::TYPE_2D_ARRAY | vk::ImageViewType::TYPE_1D_ARRAY => "2d-array",
        _ => "2d",
    }
}

pub fn gpu_view(v: &View, im: &Image, swizzle: &str) -> u32 {
    let Some(t) = fmt::tex(v.format) else { return 0 };
    let aspect = if v.aspect == vk::ImageAspectFlags::DEPTH && im.tex.is_some_and(|t| t.stencil) {
        "depth-only"
    } else if v.aspect == vk::ImageAspectFlags::STENCIL && im.tex.is_some_and(|t| t.depth) {
        "stencil-only"
    } else {
        "all"
    };
    let dim = if im.ty == vk::ImageType::TYPE_1D && im.mips == 1 && !im.tex.is_some_and(|t| t.render) {
        "1d"
    } else if v.ty == vk::ImageViewType::TYPE_1D {
        "2d"
    } else {
        view_dim(v.ty)
    };
    let id = new_id();
    json(
        op::VIEW,
        &serde_json::json!({
            "id": id, "tex": im.id, "format": t.name, "dim": dim, "baseMip": v.base_mip, "mips": v.mips,
            "baseLayer": v.base_layer, "layers": v.layers, "aspect": aspect, "swizzle": swizzle,
        }),
    );
    id
}

pub fn gpu_sampler(ci: &vk::SamplerCreateInfo) -> u32 {
    let filt = |f: vk::Filter| if f == vk::Filter::LINEAR { "linear" } else { "nearest" };
    let addr = |a: vk::SamplerAddressMode| match a {
        vk::SamplerAddressMode::REPEAT => "repeat",
        vk::SamplerAddressMode::MIRRORED_REPEAT | vk::SamplerAddressMode::MIRROR_CLAMP_TO_EDGE => "mirror-repeat",
        _ => "clamp-to-edge",
    };
    let linear_all = ci.mag_filter == vk::Filter::LINEAR
        && ci.min_filter == vk::Filter::LINEAR
        && ci.mipmap_mode == vk::SamplerMipmapMode::LINEAR;
    let id = new_id();
    let mut j = serde_json::json!({
        "id": id,
        "magFilter": filt(ci.mag_filter), "minFilter": filt(ci.min_filter),
        "mipmapFilter": if ci.mipmap_mode == vk::SamplerMipmapMode::LINEAR { "linear" } else { "nearest" },
        "addressModeU": addr(ci.address_mode_u), "addressModeV": addr(ci.address_mode_v),
        "addressModeW": addr(ci.address_mode_w),
        "lodMinClamp": ci.min_lod.max(0.0), "lodMaxClamp": ci.max_lod.clamp(0.0, 32.0).max(ci.min_lod.max(0.0)),
        "maxAnisotropy": if ci.anisotropy_enable != 0 && linear_all { ci.max_anisotropy.clamp(1.0, 16.0).floor() as u32 } else { 1 },
    });
    if ci.compare_enable != 0 {
        j["compare"] = compare(ci.compare_op).into();
    }
    json(op::SAMPLER, &j);
    id
}

pub fn compare(c: vk::CompareOp) -> &'static str {
    match c {
        vk::CompareOp::NEVER => "never",
        vk::CompareOp::LESS => "less",
        vk::CompareOp::EQUAL => "equal",
        vk::CompareOp::LESS_OR_EQUAL => "less-equal",
        vk::CompareOp::GREATER => "greater",
        vk::CompareOp::NOT_EQUAL => "not-equal",
        vk::CompareOp::GREATER_OR_EQUAL => "greater-equal",
        _ => "always",
    }
}

//! NVN host-side object layouts and the command record format: written by the record layer (`nvn::*`), read by the
//! replay (`vk::exec`).
//!
//! Objects are stored in place, in the NVN object memory the game allocated, and never exceed the size the game's SDK
//! reserves (the `fits!` assertions; newer SDKs shrank NVNtexture to 0x80, NVNtextureView to 16 bytes, and a game's
//! NVNprogram can be smaller than nvn.h's 0xC0). Some games touch the bytes directly: NVNcommandBuffer +0x10 belongs
//! to inlined SDK code, NVNbuffer offset and size live at +0x18 and +0x20, and a sampler's whole state lies from byte 8.
#![allow(non_camel_case_types, dead_code)]

use super::types::*;
use crate::tex::layout::{TexDesc, TexLayout};
use core::mem::size_of;

macro_rules! fits {
    ($t:ty, $n:expr) => {
        const _: () = assert!(
            size_of::<$t>() <= $n,
            concat!(stringify!($t), " exceeds the size the game reserves")
        );
    };
}

pub const fn magic(a: u8, b: u8, c: u8, d: u8) -> u32 {
    a as u32 | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}
pub const MAGIC_DEV: u32 = magic(b'D', b'E', b'V', 0);
pub const MAGIC_QUEUE: u32 = magic(b'Q', b'U', b'E', b'U');
pub const MAGIC_POOL: u32 = magic(b'P', b'O', b'O', b'L');
pub const MAGIC_TEX: u32 = magic(b'T', b'E', b'X', 0);
pub const MAGIC_SAMP: u32 = magic(b'S', b'M', b'P', 0);
pub const MAGIC_PROG: u32 = magic(b'P', b'R', b'O', b'G');
pub const MAGIC_SYNC: u32 = magic(b'S', b'Y', b'N', b'C');
pub const MAGIC_EVENT: u32 = magic(b'E', b'V', b'N', b'T');
pub const MAGIC_WIN: u32 = magic(b'W', b'I', b'N', b'D');
pub const MAGIC_CMD: u32 = magic(b'C', b'M', b'D', b'B');
pub const MAGIC_BUF: u32 = magic(b'B', b'U', b'F', 0);

#[repr(C)]
pub struct dev_builder_t {
    pub flags: u32,
}
fits!(dev_builder_t, 0x40);

#[repr(C)]
pub struct dev_t {
    pub magic: u32,
    pub flags: u32,
    pub origin: u32,
    pub depth_mode: u32,
    pub dbg_cb: u64,
    pub dbg_data: u64,
}
fits!(dev_t, 0x3000);

pub const QB_CMD: usize = 0;
pub const QB_COMP: usize = 1;
pub const QB_CTL: usize = 2;
pub const QB_RT: usize = 3;
#[repr(C)]
pub struct queue_builder_t {
    pub dev: *mut NVNdevice,
    pub flags: u32,
    pub sz: [u32; 4],
    pub flush: u32,
    pub set: [u8; 4],
    /// NVN2 queue priority (SetDefaults: 100, NORMAL)
    pub prio: i32,
    pub mem: *mut u8,
    /// stored as 32 bits, as the NVN2 driver does
    pub mem_sz: u32,
    /// SetCommandFlushThreshold was called (NVN2's getter returns it)
    pub flush_set: u8,
    /// NVN2: the primary queue of a subcontext queue
    pub primary: *mut NVNqueue,
}
fits!(queue_builder_t, 0x40);

#[repr(C)]
pub struct queue_t {
    pub magic: u32,
    pub dev: *mut NVNdevice,
    pub submits: u64,
    /// the window origin at the queue's creation: lower left (NVN's default) flips the picture at present
    pub origin_ll: i32,
    pub serial: u64,
}
fits!(queue_t, 0x2000);

#[repr(C)]
pub struct pool_builder_t {
    pub dev: *mut NVNdevice,
    pub mem: *mut u8,
    pub size: u64,
    pub flags: u32,
}
fits!(pool_builder_t, 0x40);

#[repr(C)]
pub struct pool_t {
    pub magic: u32,
    pub flags: u32,
    /// the pool's storage: its CPU address, which is also its GPU address
    pub mem: *mut u8,
    pub size: u64,
    pub vp: *mut u8,
    pub vmode: i32,
    pub serial: u64,
}
fits!(pool_t, 0x100);

/// Texture / sampler descriptor pool.
#[repr(C)]
pub struct dpool_t {
    pub pool: *const pool_t,
    pub off: i64,
    pub n: i32,
}
fits!(dpool_t, 0x20);

#[repr(C)]
pub struct buf_builder_t {
    pub dev: *mut NVNdevice,
    pub pool: *const pool_t,
    pub off: i64,
    pub size: u64,
}
fits!(buf_builder_t, 0x40);

#[repr(C)]
pub struct buf_t {
    pub magic: u32,
    pub pool: *const pool_t,
    pub reserved: [u8; 8],
    pub off: i64,
    pub size: u64,
}
fits!(buf_t, 0x30);
const _: () = assert!(
    core::mem::offset_of!(buf_t, off) == 0x18 && core::mem::offset_of!(buf_t, size) == 0x20
);

#[repr(C)]
pub struct tex_builder_t {
    pub dev: *mut NVNdevice,
    pub d: TexDesc,
    pub pool: *mut pool_t,
    pub off: i64,
    pub has_layout: u8,
    pub layout: [u8; 8],
    pub pkg_data: *const u8,
    /// bit 0: swizzle set, bit 1: stencil mode
    pub bits: u8,
    pub swz: u16,
    /// TextureBuilderSetGLTextureName (GL interop; the host has none)
    pub gl_name: u32,
}
fits!(tex_builder_t, 0x80);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct tex_t {
    pub magic: u32,
    pub d: TexDesc,
    pub has_layout: u8,
    /// bit 0: swizzle set (TextureBuilderSetSwizzle), bit 1: reads the stencil of its depth-stencil storage
    pub bits: u8,
    /// r | g << 3 | b << 6 | a << 9 (NVNtextureSwizzle values)
    pub swz: u16,
    pub l: TexLayout,
    pub mem: *mut u8,
    pub pool: *mut pool_t,
    pub off: i64,
    pub layout: [u8; 8],
    /// initialization serial: the replay finds the host image by it (a game may copy the object by value)
    pub serial: u64,
}
fits!(tex_t, 0x80);
const _: () = assert!(size_of::<tex_t>() == 0x80);

// A snapshot of the game's object: its pointers name game memory that outlives the snapshot's uses.
unsafe impl Send for tex_t {}
unsafe impl Sync for tex_t {}

impl tex_t {
    pub fn swizzle(&self) -> u16 {
        if self.bits & 1 != 0 {
            self.swz
        } else {
            2 | 3 << 3 | 4 << 6 | 5 << 9
        }
    }
    pub fn valid(&self) -> bool {
        self.magic == MAGIC_TEX
    }
}

/// 16 bytes (newer SDKs keep NVNtextureView in a 16-byte stack slot).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash)]
pub struct view_t {
    /// bit 0 levels, 1 layers, 2 format, 3 target, 4 swizzle, 5 depth-stencil mode set; bits 6-7 the mode
    pub bits: u8,
    pub target: u8,
    pub base_level: u8,
    pub nlevels: u8,
    pub format: u16,
    pub min_layer: u16,
    pub nlayers: u16,
    pub swz: [u8; 4],
    pub _rsv: u16,
}
fits!(view_t, 0x10);
const _: () = assert!(size_of::<view_t>() == 16);
pub const VW_LEVELS: u8 = 1;
pub const VW_LAYERS: u8 = 2;
pub const VW_FORMAT: u8 = 4;
pub const VW_TARGET: u8 = 8;
pub const VW_SWIZZLE: u8 = 16;
pub const VW_DSMODE: u8 = 32;

/// Sampler parameters (one copy shared by builder and object).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct samp_par {
    pub minf: u8,
    pub magf: u8,
    pub ws: u8,
    pub wt: u8,
    pub wr: u8,
    pub cmp_mode: u8,
    pub cmp_func: u8,
    pub reduction: u8,
    pub lod_min: f32,
    pub lod_max: f32,
    pub lod_bias: f32,
    pub aniso: f32,
    pub border: [f32; 4],
}
const _: () = assert!(size_of::<samp_par>() == 40);
#[repr(C)]
pub struct samp_builder_t {
    pub dev: *mut NVNdevice,
    pub p: samp_par,
    /// SamplerBuilderSetLodSnap (kept for its getter; Vulkan has no LOD snap)
    pub lod_snap: f32,
}
fits!(samp_builder_t, 0x60);
/// All state from byte 8 on: games hash a sampler's bytes from there.
#[repr(C)]
pub struct samp_t {
    pub magic: u32,
    pub zero: u32,
    pub p: samp_par,
}
fits!(samp_t, 0x30);
const _: () = assert!(core::mem::offset_of!(samp_t, p) == 8);

/// The per-stage shader keys are not kept here (a game's NVNprogram may be smaller than the header's): see
/// `nvn::prog` and its side table by serial.
#[repr(C)]
pub struct prog_t {
    pub magic: u32,
    pub nstage: u32,
    pub sd: [NVNshaderData; 6],
    pub serial: u64,
}
fits!(prog_t, 0x70);

/// fenced 0: never fenced since Initialize, 1: a queue fence, 2: a window acquire
#[repr(C)]
pub struct sync_t {
    pub magic: u32,
    pub fenced: u32,
    pub fence: u64,
}
fits!(sync_t, 0x40);

#[repr(C)]
pub struct ev_builder_t {
    pub pool: *const pool_t,
    pub off: i64,
}
fits!(ev_builder_t, 0x20);
#[repr(C)]
pub struct ev_t {
    pub magic: u32,
    pub val: *mut u32,
    pub pool: *const pool_t,
    pub off: i64,
}
fits!(ev_t, 0x40);

#[repr(C)]
pub struct win_builder_t {
    pub dev: *mut NVNdevice,
    pub native: u64,
    pub ntex: i32,
    pub interval: i32,
    pub tex: [*mut NVNtexture; 4],
    pub nactive: i32,
}
fits!(win_builder_t, 0x40);

#[repr(C)]
pub struct win_t {
    pub magic: u32,
    pub native: u64,
    pub ntex: i32,
    pub interval: i32,
    pub next: i32,
    pub crop: [i32; 4],
    pub tex: [*mut NVNtexture; 4],
    pub presents: u64,
    /// pacer target (ns)
    pub pace: u64,
    /// the present serial of each texture's latest present: AcquireTexture hands it out only after that one is done
    pub ticket: [u64; 4],
    pub nactive: i32,
    pub follow: i32,
    /// NVN2 HDR display metadata (red, green, blue, white xy; min, max luminance) and content metadata
    pub hdr_disp: [f32; 10],
    pub hdr_content: [f32; 2],
}
fits!(win_t, 384);

/// NVN2's window builder state past NVN1's 0x40 bytes (NVN2 reserves 0x80): at builder + 0x40, touched only when the
/// game is a Switch 2 game (`device::SWITCH2`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct win_builder2_t {
    pub color_space: i32,
    pub hdr_disp: [f32; 10],
    pub hdr_content: [f32; 2],
}
pub const WIN_BUILDER2_OFF: usize = 0x40;
const _: () = assert!(WIN_BUILDER2_OFF + size_of::<win_builder2_t>() <= 0x80);
/// The NVN2 driver's HDR defaults (Rec.709 primaries, D65 white, 0.03 to 1000 nits; content 200 / 60).
pub const HDR_DISPLAY_DEFAULT: [f32; 10] = [
    0.64, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.329, 0.03, 1000.0,
];
pub const HDR_CONTENT_DEFAULT: [f32; 2] = [200.0, 60.0];

/// Command buffer: records go into the command memory the game gives (AddCommandMemory), the list that strings a
/// recording's pieces together into its control memory (AddControlMemory). The handle is that list's address.
#[repr(C)]
pub struct cmd_t {
    pub magic: u32,
    pub recording: u32,
    pub cur: *mut u8,
    /// +0x10 is the game's: its inlined SDK code writes a byte here
    pub nvn_flag10: u64,
    pub end: *mut u8,
    pub begin: *mut u8,
    pub cb: u64,
    pub cb_data: u64,
    pub dev: *mut NVNdevice,
    pub used: u64,
    pub copy_row: i64,
    pub copy_image: i64,
    pub cmd_size: usize,
    pub ctl_mem: *mut u8,
    pub ctl_size: u32,
    pub ctl_used: u32,
    pub ctl_q: [u64; 2],
    pub ctl_list: *mut u8,
    pub chunk: *mut u8,
    pub rec_id: u32,
    pub own_back: u32,
}
fits!(cmd_t, 0xA0);
const _: () = assert!(core::mem::offset_of!(cmd_t, nvn_flag10) == 0x10);

// ---------- State objects ----------
/// blend_en: 8 bits, logic_op: 4, alpha_func: 4
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct color_st(pub u32);
impl color_st {
    pub fn blend_en(self) -> u32 {
        self.0 & 0xff
    }
    pub fn logic_op(self) -> u32 {
        self.0 >> 8 & 0xf
    }
    pub fn alpha_func(self) -> u32 {
        self.0 >> 12 & 0xf
    }
    pub fn new(blend_en: u32, logic_op: u32, alpha_func: u32) -> Self {
        color_st(blend_en & 0xff | (logic_op & 0xf) << 8 | (alpha_func & 0xf) << 12)
    }
}
/// 4 bits per target: r=1 g=2 b=4 a=8
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct chmask_st(pub u32);

/// target:3 src_rgb:7 dst_rgb:7 src_a:7 dst_a:7 eq_rgb:3 eq_a:3 adv:6 adv_npm:1 adv_ovl:2 adv_dst_free:1
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct blend_st(pub u64);
impl blend_st {
    const F: [(u32, u32); 11] = [
        (0, 3),
        (3, 7),
        (10, 7),
        (17, 7),
        (24, 7),
        (31, 3),
        (34, 3),
        (37, 6),
        (43, 1),
        (44, 2),
        (46, 1),
    ];
    pub fn get(self, i: usize) -> u32 {
        let (o, w) = Self::F[i];
        (self.0 >> o & ((1u64 << w) - 1)) as u32
    }
    pub fn set(&mut self, i: usize, v: u32) {
        let (o, w) = Self::F[i];
        let m = ((1u64 << w) - 1) << o;
        self.0 = (self.0 & !m) | ((v as u64) << o & m);
    }
}
pub mod blend_field {
    pub const TARGET: usize = 0;
    pub const SRC_RGB: usize = 1;
    pub const DST_RGB: usize = 2;
    pub const SRC_A: usize = 3;
    pub const DST_A: usize = 4;
    pub const EQ_RGB: usize = 5;
    pub const EQ_A: usize = 6;
    pub const ADV: usize = 7;
}

/// `bits`: MS_* flags. `sl`: programmable sample locations, one byte per sample, x | y << 4 in 1/16 pixel.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct ms_st {
    pub enable: u8,
    pub samples: u8,
    pub a2c: u8,
    pub a2c_dither: u8,
    pub raster_samples: u8,
    pub cov_mod: u8,
    pub cov2color_out: u8,
    pub bits: u8,
    pub sl: [u8; 16],
}
fits!(ms_st, 0x18);
pub const MS_COV2COLOR: u8 = 1;
pub const MS_SL_EN: u8 = 2;
pub const MS_SL_GRID: u8 = 4;
pub const MS_COV_TRUNC: u8 = 8;

/// cull:2 front:1 mode:2 offset_en:3
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct poly_st(pub u32);
impl poly_st {
    pub fn cull(self) -> u32 {
        self.0 & 3
    }
    pub fn front(self) -> u32 {
        self.0 >> 2 & 1
    }
    pub fn mode(self) -> u32 {
        self.0 >> 3 & 3
    }
    pub fn offset_en(self) -> u32 {
        self.0 >> 5 & 7
    }
    pub fn new(cull: u32, front: u32, mode: u32, offset_en: u32) -> Self {
        poly_st(cull & 3 | (front & 1) << 2 | (mode & 3) << 3 | (offset_en & 7) << 5)
    }
}

/// dt:1 dw:1 df:4 st:1 sf_f:4 sf_b:4 fail_f:4 zfail_f:4 zpass_f:4 fail_b:4 zfail_b:4 zpass_b:4
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct ds_st(pub u64);
impl ds_st {
    const F: [(u32, u32); 12] = [
        (0, 1),
        (1, 1),
        (2, 4),
        (6, 1),
        (7, 4),
        (11, 4),
        (15, 4),
        (19, 4),
        (23, 4),
        (27, 4),
        (31, 4),
        (35, 4),
    ];
    pub fn get(self, i: usize) -> u32 {
        let (o, w) = Self::F[i];
        (self.0 >> o & ((1u64 << w) - 1)) as u32
    }
    pub fn set(&mut self, i: usize, v: u32) {
        let (o, w) = Self::F[i];
        let m = ((1u64 << w) - 1) << o;
        self.0 = (self.0 & !m) | ((v as u64) << o & m);
    }
}
pub mod ds_field {
    pub const DT: usize = 0;
    pub const DW: usize = 1;
    pub const DF: usize = 2;
    pub const ST: usize = 3;
    pub const SF_F: usize = 4;
    pub const SF_B: usize = 5;
    pub const FAIL_F: usize = 6;
    pub const ZFAIL_F: usize = 7;
    pub const ZPASS_F: usize = 8;
    pub const FAIL_B: usize = 9;
    pub const ZFAIL_B: usize = 10;
    pub const ZPASS_B: usize = 11;
}

/// fmt:8 offset:19 stream:5
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct va_st(pub u32);
impl va_st {
    pub fn fmt(self) -> u32 {
        self.0 & 0xff
    }
    pub fn offset(self) -> u32 {
        self.0 >> 8 & 0x7ffff
    }
    pub fn stream(self) -> u32 {
        self.0 >> 27
    }
    pub fn new(fmt: u32, offset: u32, stream: u32) -> Self {
        va_st(fmt & 0xff | (offset & 0x7ffff) << 8 | (stream & 0x1f) << 27)
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct vs_st {
    pub stride: u32,
    pub divisor: u32,
}

// ---------- Command records: {op, size (header included, 8-aligned)} + payload ----------
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct op_hdr {
    pub op: u32,
    pub size: u32,
}

// Control list entries: bits 63-62 kind; bit 61 last; bits 60-44 len/8; bits 43-0 addr/8.
pub const CTL_CHUNK: u32 = 1;
pub const CTL_LINK: u32 = 3;
pub const CTL_LAST: u64 = 1 << 61;
pub const CTL_ONE: u64 = ((1u64 << 17) - 1) * 8;
pub const CTL_CHUNK_MAX: u64 = CTL_ONE - 8;
pub fn ctl_e(kind: u32, addr: *const u8, len: u64) -> u64 {
    (kind as u64) << 62 | (len >> 3) << 44 | (addr as u64) >> 3
}
pub fn ctl_kind(e: u64) -> u32 {
    (e >> 62) as u32
}
pub fn ctl_addr(e: u64) -> *const u8 {
    ((e & ((1u64 << 44) - 1)) << 3) as *const u8
}
pub fn ctl_len(e: u64) -> u64 {
    (e >> 44 & ((1u64 << 17) - 1)) << 3
}
/// # Safety
/// A CTL_ONE entry's address must hold a record header.
pub unsafe fn ctl_chunk_bytes(e: u64) -> usize {
    let mut n = ctl_len(e);
    if n == CTL_ONE {
        let h = core::ptr::read_unaligned(ctl_addr(e) as *const op_hdr);
        n = h.size as u64;
    }
    n as usize
}

/// Compact records: forms only command memory holds (as small as the driver's command where the header allows);
/// the queue's flatten expands them into canonical records, so replay never sees them.
pub mod opc {
    pub const BASE: u32 = 0x80;
    pub const DRAW: u32 = BASE;
    pub const DRAW_INST: u32 = BASE + 1;
    pub const DRAW_IND: u32 = BASE + 2;
    pub const DRAW_IDX: u32 = BASE + 3;
    pub const DRAW_IDX_INST: u32 = BASE + 4;
    pub const DRAW_IDX_IND: u32 = BASE + 5;
    pub const VIEWPORTS: u32 = BASE + 6;
    pub const STENCIL: u32 = BASE + 7;
    pub const HANDLES: u32 = BASE + 8;
    pub const BUFS: u32 = BASE + 9;
    pub const VBUFS: u32 = BASE + 10;
    pub const VATTR: u32 = BASE + 11;
    pub const VSTREAM: u32 = BASE + 12;
    pub const UBO: u32 = BASE + 13;
    pub const STATE: u32 = BASE + 14;
    pub const STATE8: u32 = BASE + 15;
    pub const BARRIER: u32 = BASE + 16;
    pub const PRIM_RESTART: u32 = BASE + 17;
    pub const SCISSOR: u32 = BASE + 18;
}
pub const REC_EXPAND_MAX: usize = 4096;
pub fn rec_compact(op: u32) -> bool {
    (op & 0xff) >= opc::BASE
}

pub mod op {
    pub const END: u32 = 0;
    pub const JUMP: u32 = 1;
    pub const BEGIN: u32 = 2;
    pub const CALL: u32 = 3;
    pub const SIGNAL_EVENT: u32 = 4;
    pub const REPORT_COUNTER: u32 = 5;
    pub const COPY_B2B: u32 = 6;
    pub const UPDATE_UB: u32 = 7;
    pub const SET_RT: u32 = 8;
    pub const CLEAR_COLOR: u32 = 9;
    pub const CLEAR_DS: u32 = 10;
    pub const VIEWPORTS: u32 = 11;
    pub const SCISSOR: u32 = 12;
    pub const DEPTH_RANGE: u32 = 13;
    pub const BIND_PROGRAM: u32 = 14;
    pub const BIND_STATE: u32 = 15;
    pub const BIND_VATTR: u32 = 16;
    pub const BIND_VSTREAM: u32 = 17;
    pub const BIND_VBUF: u32 = 18;
    pub const BIND_UBO: u32 = 19;
    pub const BIND_SSBO: u32 = 20;
    pub const BIND_TEX: u32 = 21;
    pub const BIND_SAMP: u32 = 22;
    pub const STENCIL: u32 = 23;
    pub const POLY_OFFSET: u32 = 24;
    pub const DRAW: u32 = 25;
    pub const DRAW_IDX: u32 = 26;
    pub const DISPATCH: u32 = 27;
    pub const BARRIER: u32 = 28;
    pub const SET_TEX_POOL: u32 = 29;
    pub const SET_SAMP_POOL: u32 = 30;
    pub const COPY_B2T: u32 = 31;
    pub const COPY_STRIDE: u32 = 32;
    pub const BLEND_COLOR: u32 = 33;
    pub const COPY_T2B: u32 = 34;
    pub const BIND_IMG: u32 = 35;
    pub const COPY_T2T: u32 = 36;
    pub const RESET_COUNTER: u32 = 37;
    pub const CLEAR_TEX: u32 = 38;
    pub const CLEAR_BUF: u32 = 39;
    pub const DEPTH_CLAMP: u32 = 40;
    pub const RENDER_COND: u32 = 41;
    pub const FENCE_SYNC: u32 = 42;
    pub const TF_BIND: u32 = 43;
    pub const TF_STATE: u32 = 44;
    pub const RAST_DISCARD: u32 = 45;
    pub const VP_SWIZZLE: u32 = 46;
    pub const BIND_TEX1: u32 = 47;
    pub const PRIM_RESTART: u32 = 48;
    pub const BIND_UBO_SMALL: u32 = 49;
    pub const BIND_VBUF1: u32 = 50;
    pub const DRAW_TEX: u32 = 51;
    pub const ALPHA_REF: u32 = 52;
    pub const PUSH_CONST: u32 = 53;
    pub const DLSS: u32 = 54;
    pub const MULTI_DRAW: u32 = 55;
    pub const WAIT_EVENT: u32 = 56;
    pub const WAIT_SYNC: u32 = 57;
    pub const PATCH_SIZE: u32 = 58;
    pub const LINE_WIDTH: u32 = 59;
    pub const BUFS_DEFERRED: u32 = 60;
    pub const DEPTH_BOUNDS: u32 = 61;
    pub const SAMPLE_MASK: u32 = 62;
    pub const POINT_SIZE: u32 = 63;
    pub const TESS_LEVELS: u32 = 65;
    pub const MESH: u32 = 66;
    pub const REPORT_VALUE: u32 = 67;
    pub const DOWNSAMPLE: u32 = 68;
    pub const RESOLVE_DEPTH: u32 = 69;
    pub const PROVOKING: u32 = 70;
    pub const CONS_RASTER: u32 = 71;
    pub const COUNT: u32 = 80;
}

// ---- payloads (immediately after op_hdr) ----
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_set_rt {
    pub n: i32,
    pub color: [*const tex_t; 8],
    pub cview: [view_t; 8],
    pub has_cview: [u8; 8],
    pub depth: *const tex_t,
    pub dview: view_t,
    pub has_dview: u8,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_clear_color {
    pub index: i32,
    pub mask: i32,
    pub rgba: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_clear_ds {
    pub depth: f32,
    pub depth_mask: i32,
    pub stencil: i32,
    pub stencil_mask: i32,
}
/// The first viewport's unused slot [4] carries the recording origin (1 + NVNwindowOriginMode; 0 = the device's).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_viewports {
    pub first: i32,
    pub n: i32,
    pub v: [[f32; 6]; 16],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_scissor {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_depth_range {
    pub n: f32,
    pub f: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_prim_restart {
    pub on: u32,
    pub index: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_bind_program {
    pub prog: *const prog_t,
    pub stages: i32,
}
/// kind: 0 blend 1 color 2 chmask 3 ms 4 poly 5 ds; recorded with the state object's own bytes only
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_bind_state {
    pub kind: i32,
    pub bytes: [u8; 0x18],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_bind_vattr {
    pub first: i32,
    pub n: i32,
    pub st: [u32; 32],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_bind_vstream {
    pub first: i32,
    pub n: i32,
    pub st: [vs_st; 16],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_bind_vbuf {
    pub first: i32,
    pub n: i32,
    pub r: [NVNbufferRange; 16],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_bind_vbuf1 {
    pub index: u32,
    pub size: u32,
    pub addr: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_bind_buf {
    pub stage: i32,
    pub index: i32,
    pub addr: u64,
    pub size: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_bind_ubo_small {
    pub stage: u16,
    pub index: u16,
    pub size: u32,
    pub addr: u64,
}
/// Separate texture / sampler / image handles (the low 32 bits the driver writes into c2)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_bind_handles {
    pub stage: i32,
    pub first: i32,
    pub n: i32,
    pub defer: i32,
    pub src: u64,
    pub h: [u64; 32],
}
/// nvnCommandBufferBindTexture: the handle 16 bytes from the record's start, where an engine that patches recorded
/// commands writes a new one.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_bind_tex1 {
    pub stage: i32,
    pub slot: i32,
    pub h: u64,
}
/// Texture slots of a stage: 0-127 BindTexture's combined handles, 128-255 the separate texture binds.
pub const SEPTEX_SLOT0: i32 = 128;
pub const RESOURCE_STAGES: i32 = 7;
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_stencil {
    pub face: i32,
    pub value_mask: i32,
    pub mask: i32,
    pub r#ref: i32,
    pub which: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_poly_offset {
    pub factor: f32,
    pub units: f32,
    pub clamp: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct p_draw {
    pub prim: i32,
    pub first: i32,
    pub count: i32,
    pub instances: i32,
    pub base_instance: i32,
    pub ind: u64,
    pub mdi_cnt: u64,
    pub mdi_i: i32,
    pub pad: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct p_draw_idx {
    pub prim: i32,
    pub itype: i32,
    pub count: i32,
    pub base_vertex: i32,
    pub base_instance: i32,
    pub instances: i32,
    pub ibuf: u64,
    pub ind: u64,
    pub mdi_cnt: u64,
    pub mdi_i: i32,
    pub pad: i32,
}
pub const P_DRAW_IND_OFF: usize = core::mem::offset_of!(p_draw, ind);
pub const P_DRAW_IDX_IND_OFF: usize = core::mem::offset_of!(p_draw_idx, ind);
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_dispatch {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub caller: u64,
    pub ind: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_set_pool {
    pub pool: *const dpool_t,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_copy_b2t {
    pub src: u64,
    pub tex: *const tex_t,
    pub view: view_t,
    pub has_view: u8,
    pub r: NVNcopyRegion,
    pub flags: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_copy_t2t {
    pub src: *const tex_t,
    pub dst: *const tex_t,
    pub sv: view_t,
    pub dv: view_t,
    pub has_sv: u8,
    pub has_dv: u8,
    pub sr: NVNcopyRegion,
    pub dr: NVNcopyRegion,
    pub flags: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct p_clear_tex {
    pub tex: *const tex_t,
    pub view: view_t,
    pub has_view: u8,
    pub is_uint: u8,
    pub r: NVNcopyRegion,
    pub mask: i32,
    pub c: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_copy_strides {
    pub row: i64,
    pub image: i64,
}
pub const fn snap_off(n: usize) -> usize {
    (n + 7) & !7
}
pub const COPY_STRIDES_OFF: usize = snap_off(size_of::<p_copy_b2t>()) + size_of::<tex_t>();
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_clear_buf {
    pub addr: u64,
    pub size: u64,
    pub value: u32,
    pub _pad: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_blend_color {
    pub rgba: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_line_width {
    pub w: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_depth_clamp {
    pub on: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_rast_discard {
    pub on: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct p_patch_size {
    pub n: i32,
}

/// Reads a payload struct from a record (records are 8-aligned, but read unaligned anyway: they live in game memory).
/// # Safety
/// `p` must hold at least `size_of::<T>()` bytes, or the struct must tolerate the shorter record (draws).
pub unsafe fn rd<T: Copy>(p: *const u8) -> T {
    core::ptr::read_unaligned(p as *const T)
}

// ---------- NVN2 objects (Switch 2 SDK) ----------
/// NVN2 program builder (the game reserves 0x80). The shader and module arrays are the game's: NVN keeps the pointers
/// until nvnProgramInitializeFromBuilder.
#[repr(C)]
pub struct prog_builder_t {
    pub dev: *mut NVNdevice,
    pub sample_shading: i32,
    pub nhash: i32,
    pub hash: *const u8,
    pub nshader: i32,
    pub nrt: i32,
    pub shader: *const NVNshaderData,
    pub rt: *const NVNshaderData,
    pub nlink: i32,
    pub max_trace: i32,
    pub link: *const u64,
    pub max_callable: i32,
    pub rt_flags: u32,
}
fits!(prog_builder_t, 0x80);

/// DLSS parameter texture slots (`dlss_params_t.tex`).
pub const DLSS_COLOR: usize = 0;
pub const DLSS_DEPTH: usize = 1;
pub const DLSS_MOTION: usize = 2;
pub const DLSS_OUTPUT: usize = 3;
pub const DLSS_BIAS: usize = 4;
pub const DLSS_EXPOSURE: usize = 5;
pub const DLSS_TEXTURES: usize = 6;
/// The DLSS network builder (the game reserves 0xC0).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct dlss_builder_t {
    pub dev: *mut NVNdevice,
    pub w: i32,
    pub h: i32,
    pub method: i32,
    pub preset: i32,
    pub flags: u32,
    pub max_eval: i32,
    pub tex_pool: *const NVNtexturePool,
    pub mem: *mut u8,
    pub mem_size: u64,
    pub tex_ids: [i32; 32],
}
fits!(dlss_builder_t, 0xC0);
/// The DLSS network (the game reserves 0x218): what it was built with, and a serial its evaluations carry (new at each
/// Initialize: a network made again in the same memory is another network to the replay).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct dlss_net_t {
    pub magic: u32,
    pub b: dlss_builder_t,
    pub serial: u64,
}
fits!(dlss_net_t, 0x218);
pub const MAGIC_DLSS: u32 = magic(b'D', b'L', b'S', b'S');
/// DLSS evaluation parameters (the game reserves 0xC8); nvnCommandBufferEvaluateDlss reads them at its call.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct dlss_params_t {
    pub tex: [*const tex_t; DLSS_TEXTURES],
    /// bit k: slot k was given a texture view (the whole texture is used, as nxnative does)
    pub has_view: u8,
    pub reset: u8,
    pub overlay: [u8; 4],
    pub iw: i32,
    pub ih: i32,
    pub base: [[i32; 2]; DLSS_TEXTURES],
    pub mv: [f32; 2],
    pub jitter: [f32; 2],
    pub pre_exposure: f32,
    pub eval_index: i32,
}
fits!(dlss_params_t, 0xC8);

/// NVN2 device command buffer (driver: 0x30 bytes): storage for commands the GPU generates.
#[repr(C)]
pub struct dcb_t {
    pub dev: *mut NVNdevice,
    pub pool: *const pool_t,
    pub off: i64,
    pub size: u64,
    pub cmd_size: u64,
    pub comp_size: u64,
}
fits!(dcb_t, 0x30);

/// NVN2 device message buffer (driver: 0x28 bytes): a storage range shaders write printf records into.
#[repr(C)]
pub struct dmb_t {
    pub dev: *mut NVNdevice,
    pub pool: *const pool_t,
    pub off: i32,
    pub size: i32,
    /// the read cursor of BeginMessages / GetNextMessage (0 = not reading)
    pub cursor: u32,
    pub end: u32,
}
fits!(dmb_t, 0x28);

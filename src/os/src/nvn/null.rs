//! The null NVN device: what a game reads back from NVN before it renders, answered with nxnative's values and
//! object semantics (GPU address = CPU address; objects live in the game's own NVN object memory, in layouts of
//! the host's that fit the smallest sizes the SDKs reserve). Nothing is drawn: command recording calls go to the
//! logging stub, a submit is counted and logged, a window hands out its textures in turn, and every sync is
//! signalled.

use super::tex;
use crate::guest::{r32, r64, w32, w64, Args};
use crate::hle::Registry;
use nxrt::cpu::Cpu;
use nxrt::hostfn::HostFn;
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! procs {
    ($($name:ident => |$a:ident| $body:block)*) => {
        $(
            #[allow(non_snake_case, unused_mut, unused_variables)]
            unsafe fn $name(c: *mut Cpu) {
                super::count(stringify!($name));
                let mut $a = Args(&mut *c);
                $body
            }
        )*
        const PROCS: &[(&str, HostFn)] = &[$((stringify!($name), $name as HostFn)),*];
    };
}

pub fn lookup(name: &str) -> Option<HostFn> {
    PROCS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}
pub fn implements(name: &str) -> bool {
    lookup(name).is_some()
}
pub fn count() -> usize {
    PROCS.len()
}

const MAGIC_POOL: u32 = 0x4C4F_4F50; // "POOL"

/// X1 device properties (nxnative `g_devinfo`; API 53.313 for SDK 10 and older).
const DEVINFO: [i32; 0x69] = [
    53, 313, 14, 65536, 256, 8, 16, 4, 16, 32, 16, 4, 4, 4, 16, 32, 32, 256, 256, 4, 8, 512, 256, 2, 256, 4096, 131072, 16, 1536, 1024, 64, 1536, 65535, 65535, 65535, 8,
    1048576, 4096, 16, 4096, 1, 1, 1, 262144, 15, 2048, 1, 16, 1, 16, 1, 8, 8, 4, 32, 128, 32, 128, 65536, 1, 4, 16384, 134217728, 2048, 16384, 16384, 1, 1, 1, 1, 5, 1, 1,
    1, 1, 1, 1, 16, 1024, 32, 4096, 65536, 65536, 256, 16384, 262144, 4096, 1, 4, 2, 0, 4096, 16384, 4096, 128, 32, 65535, 1, 32, 256, 16, 65535, 65535, 256, 16383,
];

static TEX_SERIAL: AtomicU64 = AtomicU64::new(1);
static HANDLE_SERIAL: AtomicU64 = AtomicU64::new(1);

// Builders and objects (offsets in the game's NVN memory):
//   memory pool builder: +0 device, +8 memory, +0x10 size, +0x18 flags
//   memory pool:         +0 magic, +4 flags, +8 memory, +0x10 size
//   buffer builder:      +0 device, +8 pool, +0x10 offset, +0x18 size
//   buffer:              +0 pool memory + offset (address), +8 size, +0x10 pool, +0x18 offset
//   texture builder:     +0 device, +8 flags, +0xc target, +0x10 width, +0x14 height, +0x18 depth, +0x1c levels,
//                        +0x20 format, +0x24 samples, +0x28 stride, +0x2c packaged layout + 1, +0x30 pool, +0x38 offset
//   texture:             the builder's +8..+0x40, then +0x40 address, +0x48 storage size, +0x50 serial
//   queue builder:       +0 device, +8 command, +0xc compute, +0x10 control memory sizes, +0x18 memory, +0x20 size
//   command buffer:      +0 magic, +4 recording, +8 command memory size; +0x10 is the game's (its inlined NVN
//                        header code writes a byte there); +0x18 control memory size, +0x20 callback, +0x28 its data
//   window builder:      +0 device, +8 native window, +0x10 textures, +0x14 interval, +0x18 texture pointers [4],
//                        +0x38 active textures
//   window:              the builder's fields, +0x40 next index, +0x44 crop [4]
//   event builder:       +0 pool, +8 offset;   event: +0 address of its value word, +8 pool, +0x10 offset
fn pool_mem(pool: u64) -> u64 {
    if pool == 0 {
        0
    } else {
        r64(pool + 8)
    }
}

fn tex_desc(b: u64) -> tex::Desc {
    tex::Desc {
        flags: r32(b + 8) as i32,
        target: r32(b + 0xC) as i32,
        width: r32(b + 0x10),
        height: r32(b + 0x14),
        depth: r32(b + 0x18),
        levels: r32(b + 0x1C),
        format: r32(b + 0x20) as i32,
        samples: r32(b + 0x24) as i32,
        stride: r32(b + 0x28),
        pkg_layout: r32(b + 0x2C) as i32,
    }
}

fn tex_builder_defaults(b: u64) {
    let dev = r64(b);
    crate::guest::zero(b, 0x40);
    w64(b, dev);
    w32(b + 0xC, 1); // 2D
    w32(b + 0x10, 1);
    w32(b + 0x14, 1);
    w32(b + 0x18, 1);
    w32(b + 0x1C, 1);
}

procs! {
    // ---- device
    nvnDeviceBuilderSetDefaults => |a| { w32(a.x(0), 0); }
    nvnDeviceBuilderSetFlags => |a| { w32(a.x(0), a.w(1)); }
    nvnDeviceBuilderGetFlags => |a| { let v = r32(a.x(0)); a.ret(v as u64); }
    nvnDeviceInitialize => |a| {
        crate::log!("nvn: device {:#x} initialized (null device: nothing is drawn)", a.x(0));
        w32(a.x(0), 0x5645_444E);
        a.ret(1);
    }
    nvnDeviceFinalize => |a| {}
    nvnDeviceGetProcAddress => |a| { let n = crate::guest::cstr(a.x(1), 128); let p = super::proc_address(&n); a.ret(p); }
    nvnDeviceGetInteger => |a| {
        let w = a.w(1) as usize;
        let v = match DEVINFO.get(w) {
            Some(&v) => v,
            None => {
                crate::log_once!(format!("devint {w:#x}"), "[RED] nvnDeviceGetInteger item {w:#x}: unknown, 0");
                0
            }
        };
        crate::guest::out(a.x(2), v);
    }
    nvnDeviceGetCurrentTimestampInNanoseconds => |a| { a.ret(crate::svc::guest_ns()); }
    nvnDeviceGetTimestampInNanoseconds => |a| {
        // a counter report {u64 counter, u64 GPU timestamp (19.2 MHz)}: its time
        let p = a.x(1);
        let t = if p != 0 { r64(p + 8) } else { 0 };
        a.ret((t as u128 * 625 / 12) as u64);
    }
    nvnDeviceGetTextureHandle => |a| { a.ret(a.w(1) as u64 | (a.w(2) as u64) << 20); }
    nvnDeviceGetTexelFetchHandle => |a| { a.ret(a.w(1) as u64); }
    nvnDeviceGetImageHandle => |a| { a.ret(a.w(1) as u64); }
    nvnDeviceGetWindowOriginMode => |a| { a.ret(0); }
    nvnDeviceGetDepthMode => |a| { a.ret(0); }
    nvnDeviceSetWindowOriginMode => |a| {}
    nvnDeviceSetDepthMode => |a| {}
    nvnDeviceSetIntermediateShaderCache => |a| {}
    nvnDeviceInstallDebugCallback => |a| {}
    nvnDeviceGenerateDebugDomainId => |a| { a.ret(HANDLE_SERIAL.fetch_add(1, Ordering::Relaxed)); }
    nvnDeviceFinalizeCommandHandle => |a| {}

    // ---- memory pools
    nvnMemoryPoolBuilderSetDefaults => |a| { let b = a.x(0); w64(b + 8, 0); w64(b + 0x10, 0); w32(b + 0x18, 0x1 | 0x10); }
    nvnMemoryPoolBuilderSetDevice => |a| { w64(a.x(0), a.x(1)); }
    nvnMemoryPoolBuilderSetStorage => |a| { let b = a.x(0); w64(b + 8, a.x(1)); w64(b + 0x10, a.x(2)); }
    nvnMemoryPoolBuilderSetFlags => |a| { w32(a.x(0) + 0x18, a.w(1)); }
    nvnMemoryPoolBuilderGetFlags => |a| { let v = r32(a.x(0) + 0x18); a.ret(v as u64); }
    nvnMemoryPoolBuilderGetMemory => |a| { let v = r64(a.x(0) + 8); a.ret(v); }
    nvnMemoryPoolBuilderGetSize => |a| { let v = r64(a.x(0) + 0x10); a.ret(v); }
    nvnMemoryPoolInitialize => |a| {
        let (p, b) = (a.x(0), a.x(1));
        let (mem, size, flags) = (r64(b + 8), r64(b + 0x10), r32(b + 0x18));
        w32(p, MAGIC_POOL);
        w32(p + 4, flags);
        w64(p + 8, mem);
        w64(p + 0x10, size);
        a.ret(1);
    }
    nvnMemoryPoolFinalize => |a| { w32(a.x(0), 0); }
    nvnMemoryPoolMap => |a| { let v = pool_mem(a.x(0)); a.ret(v); }
    nvnMemoryPoolGetBufferAddress => |a| { let v = pool_mem(a.x(0)); a.ret(v); }
    nvnMemoryPoolGetSize => |a| { let v = r64(a.x(0) + 0x10); a.ret(v); }
    nvnMemoryPoolGetFlags => |a| { let v = r32(a.x(0) + 4); a.ret(v as u64); }
    nvnMemoryPoolFlushMappedRange => |a| {}
    nvnMemoryPoolInvalidateMappedRange => |a| {}
    nvnMemoryPoolSetDebugLabel => |a| {}
    nvnMemoryPoolMapVirtual => |a| { a.ret(1); }

    // ---- buffers
    nvnBufferBuilderSetDefaults => |a| { let b = a.x(0); w64(b + 8, 0); w64(b + 0x10, 0); w64(b + 0x18, 0); }
    nvnBufferBuilderSetDevice => |a| { w64(a.x(0), a.x(1)); }
    nvnBufferBuilderSetStorage => |a| { let b = a.x(0); w64(b + 8, a.x(1)); w64(b + 0x10, a.x(2)); w64(b + 0x18, a.x(3)); }
    nvnBufferInitialize => |a| {
        let (o, b) = (a.x(0), a.x(1));
        let (pool, off, size) = (r64(b + 8), r64(b + 0x10), r64(b + 0x18));
        w64(o, pool_mem(pool) + off);
        w64(o + 8, size);
        w64(o + 0x10, pool);
        w64(o + 0x18, off);
        a.ret(1);
    }
    nvnBufferFinalize => |a| {}
    nvnBufferMap => |a| { let v = r64(a.x(0)); a.ret(v); }
    nvnBufferGetAddress => |a| { let v = r64(a.x(0)); a.ret(v); }
    nvnBufferGetSize => |a| { let v = r64(a.x(0) + 8); a.ret(v); }
    nvnBufferGetMemoryPool => |a| { let v = r64(a.x(0) + 0x10); a.ret(v); }
    nvnBufferGetMemoryOffset => |a| { let v = r64(a.x(0) + 0x18); a.ret(v); }
    nvnBufferFlushMappedRange => |a| {}
    nvnBufferInvalidateMappedRange => |a| {}
    nvnBufferSetDebugLabel => |a| {}

    // ---- textures
    nvnTextureBuilderSetDefaults => |a| { tex_builder_defaults(a.x(0)); }
    nvnTextureBuilderSetDevice => |a| { w64(a.x(0), a.x(1)); }
    nvnTextureBuilderSetFlags => |a| { w32(a.x(0) + 8, a.w(1)); }
    nvnTextureBuilderSetTarget => |a| { w32(a.x(0) + 0xC, a.w(1)); }
    nvnTextureBuilderSetWidth => |a| { w32(a.x(0) + 0x10, a.w(1)); }
    nvnTextureBuilderSetHeight => |a| { w32(a.x(0) + 0x14, a.w(1)); }
    nvnTextureBuilderSetDepth => |a| { w32(a.x(0) + 0x18, a.w(1)); }
    nvnTextureBuilderSetSize1D => |a| { let b = a.x(0); w32(b + 0x10, a.w(1)); w32(b + 0x14, 1); w32(b + 0x18, 1); }
    nvnTextureBuilderSetSize2D => |a| { let b = a.x(0); w32(b + 0x10, a.w(1)); w32(b + 0x14, a.w(2)); w32(b + 0x18, 1); }
    nvnTextureBuilderSetSize3D => |a| { let b = a.x(0); w32(b + 0x10, a.w(1)); w32(b + 0x14, a.w(2)); w32(b + 0x18, a.w(3)); }
    nvnTextureBuilderSetLevels => |a| { w32(a.x(0) + 0x1C, a.w(1)); }
    nvnTextureBuilderSetFormat => |a| { w32(a.x(0) + 0x20, a.w(1)); }
    nvnTextureBuilderSetSamples => |a| { w32(a.x(0) + 0x24, a.w(1)); }
    nvnTextureBuilderSetStride => |a| { w32(a.x(0) + 0x28, a.x(1) as u32); }
    nvnTextureBuilderSetPackagedTextureLayout => |a| {
        let l = a.x(1);
        let v = if l == 0 { 0 } else { (crate::guest::r8(l) as u32 | ((crate::guest::r8(l + 1) as u32 & 0xF) << 8)) + 1 };
        w32(a.x(0) + 0x2C, v);
    }
    nvnTextureBuilderSetPackagedTextureData => |a| {}
    nvnTextureBuilderSetSwizzle => |a| {}
    nvnTextureBuilderSetDepthStencilMode => |a| {}
    nvnTextureBuilderSetStorage => |a| { let b = a.x(0); w64(b + 0x30, a.x(1)); w64(b + 0x38, a.x(2)); }
    nvnTextureBuilderGetStorageSize => |a| { let l = tex::layout(&tex_desc(a.x(0))); a.ret(l.size); }
    nvnTextureBuilderGetStorageAlignment => |a| { let l = tex::layout(&tex_desc(a.x(0))); a.ret(l.align as u64); }
    nvnTextureBuilderGetFlags => |a| { let v = r32(a.x(0) + 8); a.ret(v as u64); }
    nvnTextureBuilderGetTarget => |a| { let v = r32(a.x(0) + 0xC); a.ret(v as u64); }
    nvnTextureBuilderGetWidth => |a| { let v = r32(a.x(0) + 0x10); a.ret(v as u64); }
    nvnTextureBuilderGetHeight => |a| { let v = r32(a.x(0) + 0x14); a.ret(v as u64); }
    nvnTextureBuilderGetDepth => |a| { let v = r32(a.x(0) + 0x18); a.ret(v as u64); }
    nvnTextureBuilderGetLevels => |a| { let v = r32(a.x(0) + 0x1C); a.ret(v as u64); }
    nvnTextureBuilderGetFormat => |a| { let v = r32(a.x(0) + 0x20); a.ret(v as u64); }
    nvnTextureBuilderGetSamples => |a| { let v = r32(a.x(0) + 0x24); a.ret(v as u64); }
    nvnTextureBuilderGetStride => |a| { let v = r32(a.x(0) + 0x28); a.ret(v as u64); }
    nvnTextureBuilderGetMemoryPool => |a| { let v = r64(a.x(0) + 0x30); a.ret(v); }
    nvnTextureBuilderGetMemoryOffset => |a| { let v = r64(a.x(0) + 0x38); a.ret(v); }
    nvnTextureInitialize => |a| {
        let (t, b) = (a.x(0), a.x(1));
        let bytes = crate::guest::bytes(b + 8, 0x38).to_vec();
        crate::guest::write_bytes(t + 8, &bytes);
        let l = tex::layout(&tex_desc(b));
        w64(t + 0x40, pool_mem(r64(b + 0x30)) + r64(b + 0x38));
        w64(t + 0x48, l.size);
        w64(t + 0x50, TEX_SERIAL.fetch_add(1, Ordering::Relaxed));
        w32(t, 0x5845_5454);
        a.ret(1);
    }
    nvnTextureFinalize => |a| {}
    nvnTextureSetDebugLabel => |a| {}
    nvnTextureGetFlags => |a| { let v = r32(a.x(0) + 8); a.ret(v as u64); }
    nvnTextureGetTarget => |a| { let v = r32(a.x(0) + 0xC); a.ret(v as u64); }
    nvnTextureGetWidth => |a| { let v = r32(a.x(0) + 0x10); a.ret(v as u64); }
    nvnTextureGetHeight => |a| { let v = r32(a.x(0) + 0x14); a.ret(v as u64); }
    nvnTextureGetDepth => |a| { let v = r32(a.x(0) + 0x18); a.ret(v as u64); }
    nvnTextureGetLevels => |a| { let v = r32(a.x(0) + 0x1C); a.ret(v as u64); }
    nvnTextureGetFormat => |a| { let v = r32(a.x(0) + 0x20); a.ret(v as u64); }
    nvnTextureGetSamples => |a| { let v = r32(a.x(0) + 0x24); a.ret(v as u64); }
    nvnTextureGetStride => |a| { let v = r32(a.x(0) + 0x28); a.ret(v as u64); }
    nvnTextureGetMemoryPool => |a| { let v = r64(a.x(0) + 0x30); a.ret(v); }
    nvnTextureGetMemoryOffset => |a| { let v = r64(a.x(0) + 0x38); a.ret(v); }
    nvnTextureGetTextureAddress => |a| { let v = r64(a.x(0) + 0x40); a.ret(v); }
    nvnTextureGetStorageSize => |a| { let v = r64(a.x(0) + 0x48); a.ret(v); }
    nvnTextureViewSetDefaults => |a| { crate::guest::zero(a.x(0), 16); }
    nvnTextureCompare => |a| {
        let same = crate::guest::bytes(a.x(0) + 8, 0x38) == crate::guest::bytes(a.x(1) + 8, 0x38);
        a.ret(same as u64);
    }
    nvnTextureGetZCullStorageSize => |a| { a.ret(0); }
    nvnTextureBuilderGetZCullStorageSize => |a| { a.ret(0); }
    nvnTextureBuilderGetStorageClass => |a| { a.ret(0); }
    nvnTextureGetStorageClass => |a| { a.ret(0); }
    nvnTextureWriteTexels => |a| {}
    nvnTextureWriteTexelsStrided => |a| {}
    nvnTextureReadTexels => |a| {}
    nvnTextureReadTexelsStrided => |a| {}
    nvnTextureFlushTexels => |a| {}
    nvnTextureInvalidateTexels => |a| {}
    nvnSamplerCompare => |a| { a.ret(1); }
    nvnTextureViewSetLevels => |a| {}
    nvnTextureViewSetLayers => |a| {}
    nvnTextureViewSetFormat => |a| {}
    nvnTextureViewSetTarget => |a| {}
    nvnTextureViewSetSwizzle => |a| {}
    nvnTextureViewSetDepthStencilMode => |a| {}

    // ---- texture and sampler pools, samplers
    nvnTexturePoolInitialize => |a| { let p = a.x(0); w64(p, a.x(1)); w64(p + 8, a.x(2)); w32(p + 0x10, a.w(3)); a.ret(1); }
    nvnTexturePoolFinalize => |a| {}
    nvnTexturePoolRegisterTexture => |a| {}
    nvnTexturePoolRegisterImage => |a| {}
    nvnTexturePoolGetMemoryPool => |a| { let v = r64(a.x(0)); a.ret(v); }
    nvnTexturePoolGetMemoryOffset => |a| { let v = r64(a.x(0) + 8); a.ret(v); }
    nvnTexturePoolGetSize => |a| { let v = r32(a.x(0) + 0x10); a.ret(v as u64); }
    nvnTexturePoolSetDebugLabel => |a| {}
    nvnSamplerPoolInitialize => |a| { let p = a.x(0); w64(p, a.x(1)); w64(p + 8, a.x(2)); w32(p + 0x10, a.w(3)); a.ret(1); }
    nvnSamplerPoolFinalize => |a| {}
    nvnSamplerPoolRegisterSampler => |a| {}
    nvnSamplerPoolRegisterSamplerBuilder => |a| {}
    nvnSamplerPoolGetMemoryPool => |a| { let v = r64(a.x(0)); a.ret(v); }
    nvnSamplerPoolGetMemoryOffset => |a| { let v = r64(a.x(0) + 8); a.ret(v); }
    nvnSamplerPoolGetSize => |a| { let v = r32(a.x(0) + 0x10); a.ret(v as u64); }
    nvnSamplerPoolSetDebugLabel => |a| {}
    nvnSamplerBuilderSetDefaults => |a| {}
    nvnSamplerBuilderSetDevice => |a| {}
    nvnSamplerBuilderSetMinMagFilter => |a| {}
    nvnSamplerBuilderSetWrapMode => |a| {}
    nvnSamplerBuilderSetLodClamp => |a| {}
    nvnSamplerBuilderSetLodBias => |a| {}
    nvnSamplerBuilderSetCompare => |a| {}
    nvnSamplerBuilderSetBorderColor => |a| {}
    nvnSamplerBuilderSetMaxAnisotropy => |a| {}
    nvnSamplerBuilderSetReductionFilter => |a| {}
    nvnSamplerInitialize => |a| { a.ret(1); }
    nvnSamplerFinalize => |a| {}

    // ---- programs, syncs, events
    nvnProgramInitialize => |a| { a.ret(1); }
    nvnProgramFinalize => |a| {}
    nvnProgramSetShaders => |a| { a.ret(1); }
    nvnProgramSetDebugLabel => |a| {}
    nvnSyncInitialize => |a| { a.ret(1); }
    nvnSyncFinalize => |a| {}
    nvnSyncWait => |a| { a.ret(0); } // ALREADY_SIGNALED
    nvnSyncSetDebugLabel => |a| {}
    nvnEventBuilderSetDefaults => |a| { w64(a.x(0), 0); w64(a.x(0) + 8, 0); }
    nvnEventBuilderSetStorage => |a| { w64(a.x(0), a.x(1)); w64(a.x(0) + 8, a.x(2)); }
    nvnEventInitialize => |a| {
        let (e, b) = (a.x(0), a.x(1));
        let (pool, off) = (r64(b), r64(b + 8));
        w64(e, pool_mem(pool) + off);
        w64(e + 8, pool);
        w64(e + 0x10, off);
        a.ret(1);
    }
    nvnEventFinalize => |a| {}
    nvnEventGetValue => |a| { let p = r64(a.x(0)); let v = if p != 0 { r32(p) } else { 0 }; a.ret(v as u64); }
    nvnEventSignal => |a| { let p = r64(a.x(0)); if p != 0 { w32(p, a.w(2)); } }
    nvnEventGetMemoryPool => |a| { let v = r64(a.x(0) + 8); a.ret(v); }
    nvnEventGetMemoryOffset => |a| { let v = r64(a.x(0) + 0x10); a.ret(v); }

    // ---- queues
    nvnQueueBuilderSetDefaults => |a| { let b = a.x(0); w32(b + 8, 0x10000); w32(b + 0xC, 0x40000); w32(b + 0x10, 0x4000); }
    nvnQueueBuilderSetDevice => |a| { w64(a.x(0), a.x(1)); }
    nvnQueueBuilderSetFlags => |a| {}
    nvnQueueBuilderSetCommandMemorySize => |a| { w32(a.x(0) + 8, a.w(1)); }
    nvnQueueBuilderSetComputeMemorySize => |a| { w32(a.x(0) + 0xC, a.w(1)); }
    nvnQueueBuilderSetControlMemorySize => |a| { w32(a.x(0) + 0x10, a.w(1)); }
    nvnQueueBuilderSetCommandFlushThreshold => |a| {}
    nvnQueueBuilderGetQueueMemorySize => |a| {
        let b = a.x(0);
        let n = r32(b + 8) as u64 + r32(b + 0xC) as u64 + r32(b + 0x10) as u64;
        a.ret((n + 4095) & !4095);
    }
    nvnQueueBuilderSetQueueMemory => |a| { w64(a.x(0) + 0x18, a.x(1)); w64(a.x(0) + 0x20, a.x(2)); }
    nvnQueueInitialize => |a| {
        crate::log!("nvn: queue {:#x} initialized", a.x(0));
        w32(a.x(0), 0x5545_5551);
        a.ret(1);
    }
    nvnQueueFinalize => |a| {}
    nvnQueueGetError => |a| { if a.x(1) != 0 { crate::guest::zero(a.x(1), 0x10); } a.ret(0); }
    nvnQueueFlush => |a| {}
    nvnQueueFinish => |a| {}
    nvnQueueFenceSync => |a| {}
    nvnQueueWaitSync => |a| { a.ret(1); }
    nvnQueueSetDebugLabel => |a| {}
    nvnQueueSubmitCommands => |a| { super::note_submit(a.x(0), a.x(1)); }
    nvnQueuePresentTexture => |a| { super::note_present(a.x(1), a.w(2)); super::pace_present(); }
    nvnQueueAcquireTexture => |a| {
        let w = a.x(1);
        let n = r32(w + 0x10).clamp(1, 4);
        let i = r32(w + 0x40);
        w32(w + 0x40, (i + 1) % n);
        crate::guest::out(a.x(2), i);
        a.ret(0);
    }

    // ---- command buffers
    nvnCommandBufferInitialize => |a| { let c = a.x(0); w32(c, 0x4442_4D43); w32(c + 4, 0); w64(c + 8, 0); w64(c + 0x18, 0); w64(c + 0x20, 0); a.ret(1); }
    nvnCommandBufferFinalize => |a| {}
    nvnCommandBufferSetDebugLabel => |a| {}
    nvnCommandBufferSetMemoryCallback => |a| { w64(a.x(0) + 0x20, a.x(1)); }
    nvnCommandBufferSetMemoryCallbackData => |a| { w64(a.x(0) + 0x28, a.x(1)); }
    nvnCommandBufferAddCommandMemory => |a| { w64(a.x(0) + 8, a.x(3)); }
    nvnCommandBufferAddControlMemory => |a| { w64(a.x(0) + 0x18, a.x(2)); }
    nvnCommandBufferGetCommandMemorySize => |a| { let v = r64(a.x(0) + 8); a.ret(v); }
    nvnCommandBufferGetCommandMemoryUsed => |a| { a.ret(0); }
    nvnCommandBufferGetCommandMemoryFree => |a| { let v = r64(a.x(0) + 8); a.ret(v); }
    nvnCommandBufferGetControlMemorySize => |a| { let v = r64(a.x(0) + 0x18); a.ret(v); }
    nvnCommandBufferGetControlMemoryUsed => |a| { a.ret(0); }
    nvnCommandBufferGetControlMemoryFree => |a| { let v = r64(a.x(0) + 0x18); a.ret(v); }
    nvnCommandBufferBeginRecording => |a| { w32(a.x(0) + 4, 1); }
    nvnCommandBufferEndRecording => |a| { w32(a.x(0) + 4, 0); a.ret(0x4000_0000_0000 | HANDLE_SERIAL.fetch_add(1, Ordering::Relaxed) << 4); }
    nvnCommandBufferIsRecording => |a| { let v = r32(a.x(0) + 4); a.ret(v as u64); }
    nvnCommandBufferSignalEvent => |a| {
        // (cmd, event, mode, index, value): nothing runs on a GPU, so the value lands at once
        let p = r64(a.x(1));
        if p != 0 { w32(p, a.w(4)); }
    }

    // ---- windows
    nvnWindowBuilderSetDefaults => |a| { let b = a.x(0); let d = r64(b); crate::guest::zero(b, 0x40); w64(b, d); w32(b + 0x14, 1); }
    nvnWindowBuilderSetDevice => |a| { w64(a.x(0), a.x(1)); }
    nvnWindowBuilderSetNativeWindow => |a| { w64(a.x(0) + 8, a.x(1)); }
    nvnWindowBuilderSetTextures => |a| {
        let (b, n, t) = (a.x(0), a.w(1).min(4), a.x(2));
        w32(b + 0x10, n);
        for i in 0..n as u64 { w64(b + 0x18 + 8 * i, r64(t + 8 * i)); }
    }
    nvnWindowBuilderSetPresentInterval => |a| { w32(a.x(0) + 0x14, a.w(1)); }
    nvnWindowBuilderSetNumActiveTextures => |a| { w32(a.x(0) + 0x38, a.w(1)); }
    nvnWindowBuilderGetPresentInterval => |a| { let v = r32(a.x(0) + 0x14); a.ret(v as u64); }
    nvnWindowInitialize => |a| {
        let (w, b) = (a.x(0), a.x(1));
        let bytes = crate::guest::bytes(b, 0x40).to_vec();
        crate::guest::write_bytes(w, &bytes);
        w32(w + 0x40, 0);
        crate::log!("nvn: window {w:#x} initialized: {} textures, native window {:#x}", r32(b + 0x10), r64(b + 8));
        a.ret(1);
    }
    nvnWindowFinalize => |a| {}
    nvnWindowSetDebugLabel => |a| {}
    nvnWindowAcquireTexture => |a| {
        let w = a.x(0);
        let act = r32(w + 0x38);
        let n = if act != 0 { act } else { r32(w + 0x10) }.clamp(1, 4);
        let i = r32(w + 0x40) % n;
        w32(w + 0x40, (i + 1) % n);
        crate::guest::out(a.x(2), i);
        a.ret(0);
    }
    nvnWindowGetPresentInterval => |a| { let v = r32(a.x(0) + 0x14); a.ret(v as u64); }
    nvnWindowSetPresentInterval => |a| { w32(a.x(0) + 0x14, a.w(1)); }
    nvnWindowSetCrop => |a| { let w = a.x(0); for i in 0..4 { w32(w + 0x44 + 4 * i, a.w(1 + i as usize)); } }
    nvnWindowGetCrop => |a| { let (w, o) = (a.x(0), a.x(1)); if o != 0 { for i in 0..4 { w32(o + 4 * i, r32(w + 0x44 + 4 * i)); } } }
    nvnWindowSetNumActiveTextures => |a| { w32(a.x(0) + 0x38, a.w(1)); }
    nvnWindowGetNumActiveTextures => |a| { let w = a.x(0); let act = r32(w + 0x38); a.ret(if act != 0 { act } else { r32(w + 0x10) } as u64); }
    nvnWindowGetNumTextures => |a| { let v = r32(a.x(0) + 0x10); a.ret(v as u64); }
}

// ---- nv:: (the driver glue under NVN): nothing to do with a null device

static NV_POOL: AtomicU64 = AtomicU64::new(0);
static NV_SIZE: AtomicU64 = AtomicU64::new(0);

unsafe fn nv_init(c: *mut Cpu) {
    let a = Args(&mut *c);
    NV_POOL.store(a.x(0), Ordering::Relaxed);
    NV_SIZE.store(a.x(1), Ordering::Relaxed);
    crate::log!("nv::InitializeGraphics: pool {:.2} MB at {:#x}, recorded", a.x(1) as f64 / 1048576.0, a.x(0));
}
unsafe fn nv_fin(_c: *mut Cpu) {
    NV_POOL.store(0, Ordering::Relaxed);
}
unsafe fn nv_init_check(c: *mut Cpu) {
    if NV_POOL.load(Ordering::Relaxed) == 0 {
        nxrt::rt::guest_fatal(c, "nv::InitCheck: graphics not initialised");
    }
}
unsafe fn nv_mem_info(c: *mut Cpu) {
    let o = (*c).x[0];
    crate::guest::zero(o, 0x20); // SDK 10 and older: four counters
    let n = NV_SIZE.load(Ordering::Relaxed);
    w64(o, n);
    w64(o + 16, n);
}
unsafe fn nv_dump(_c: *mut Cpu) {
    crate::log!("nv::DumpGraphicsMemoryInfo: pool {:.2} MB, 0 used (null device)", NV_SIZE.load(Ordering::Relaxed) as f64 / 1048576.0);
}

pub fn register_nv(r: &mut Registry) {
    const WHY: &str = "NVN is the host's: the driver glue under it has nothing to do";
    r.force("_ZN2nv18InitializeGraphicsEPvm", WHY, nv_init);
    r.force("_ZN2nv16FinalizeGraphicsEv", WHY, nv_fin);
    r.force("_ZN2nv9InitCheckEv", WHY, nv_init_check);
    r.force("_ZN2nv21GetGraphicsMemoryInfoEPNS_10MemoryInfoE", WHY, nv_mem_info);
    r.force("_ZN2nv22DumpGraphicsMemoryInfoEv", WHY, nv_dump);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(f: HostFn, args: &[u64]) -> u64 {
        let mut c = Cpu::new_boxed();
        for (i, &v) in args.iter().enumerate() {
            c.x[i] = v;
        }
        unsafe { f(&mut *c) };
        c.x[0]
    }

    #[test]
    fn pools_buffers_textures() {
        let mut mem = vec![0u8; 0x10000];
        let m = mem.as_mut_ptr() as u64;
        let (pb, pool, bb, buf, tb, t) = (m, m + 0x100, m + 0x200, m + 0x300, m + 0x400, m + 0x500);
        let storage = m + 0x8000;
        call(nvnMemoryPoolBuilderSetDefaults, &[pb]);
        call(nvnMemoryPoolBuilderSetStorage, &[pb, storage, 0x8000]);
        assert_eq!(call(nvnMemoryPoolInitialize, &[pool, pb]), 1);
        assert_eq!(call(nvnMemoryPoolMap, &[pool]), storage);
        assert_eq!(call(nvnMemoryPoolGetSize, &[pool]), 0x8000);
        call(nvnBufferBuilderSetStorage, &[bb, pool, 0x100, 0x40]);
        assert_eq!(call(nvnBufferInitialize, &[buf, bb]), 1);
        assert_eq!(call(nvnBufferMap, &[buf]), storage + 0x100);
        assert_eq!(call(nvnBufferGetAddress, &[buf]), storage + 0x100);
        call(nvnTextureBuilderSetDefaults, &[tb]);
        call(nvnTextureBuilderSetSize2D, &[tb, 256, 256]);
        call(nvnTextureBuilderSetFormat, &[tb, 0x25]);
        assert_eq!(call(nvnTextureBuilderGetStorageSize, &[tb]), 0x40000);
        call(nvnTextureBuilderSetStorage, &[tb, pool, 0x1000]);
        assert_eq!(call(nvnTextureInitialize, &[t, tb]), 1);
        assert_eq!(call(nvnTextureGetWidth, &[t]), 256);
        assert_eq!(call(nvnTextureGetTextureAddress, &[t]), storage + 0x1000);
        let mut v = 0i32;
        call(nvnDeviceGetInteger, &[0, 0, &mut v as *mut i32 as u64]);
        assert_eq!(v, 53);
        call(nvnDeviceGetInteger, &[0, 0x15, &mut v as *mut i32 as u64]);
        assert_eq!(v, 512, "texture storage alignment");
        assert!(lookup("nvnQueueSubmitCommands").is_some() && lookup("nvnCommandBufferDrawArrays").is_none());
    }

    #[test]
    fn windows_rotate() {
        let mut mem = vec![0u8; 0x200];
        let m = mem.as_mut_ptr() as u64;
        let texs = [0x1000u64, 0x2000];
        call(nvnWindowBuilderSetDefaults, &[m]);
        call(nvnWindowBuilderSetTextures, &[m, 2, texs.as_ptr() as u64]);
        assert_eq!(call(nvnWindowInitialize, &[m + 0x100, m]), 1);
        let mut idx = 9i32;
        let p = &mut idx as *mut i32 as u64;
        for want in [0, 1, 0] {
            assert_eq!(call(nvnWindowAcquireTexture, &[m + 0x100, 0, p]), 0);
            assert_eq!(idx, want);
        }
    }
}

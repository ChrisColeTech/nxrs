//! Memory pools, buffers, and texture / sampler descriptor pools.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use crate::vk;
use std::sync::Mutex;

/// A pool's CPU mapping: none for CPU_NO_ACCESS and VIRTUAL pools (their address is a GPU address only).
pub fn pool_cpu_map(p: &pool_t) -> *mut u8 {
    if p.flags & (mem_flags::CPU_NO_ACCESS | mem_flags::VIRTUAL) != 0 {
        core::ptr::null_mut()
    } else {
        p.mem
    }
}

// One ID space per descriptor pool: in NVN a texture or sampler ID indexes the pool the command buffer has bound, and
// two pools may use the same IDs. The back end keeps one table of each kind, so every pool gets a base in it, in
// creation order (256-aligned); registrations go to base + ID, and the replay adds the active pool's base.
const NDPOOL: usize = 16;
struct DpoolBases {
    t: Vec<(usize, i32)>,
    next: i32,
    end: i32,
}
static TEX_BASES: Mutex<DpoolBases> = Mutex::new(DpoolBases { t: Vec::new(), next: 0, end: 0 });
static SAMP_BASES: Mutex<DpoolBases> = Mutex::new(DpoolBases { t: Vec::new(), next: 0, end: 0 });

fn dpool_add(m: &Mutex<DpoolBases>, pool: usize, n: i32, kind: &str) -> i32 {
    let mut g = m.lock().unwrap();
    let base = match g.t.iter().find(|e| e.0 == pool) {
        Some(e) => e.1,
        None => {
            if g.t.len() == NDPOOL {
                crate::log::red_once(&format!("{kind} pools exceed {NDPOOL}: later ones share the first pool's numbering"));
                return 0;
            }
            let b = g.next;
            g.t.push((pool, b));
            g.next += (n + 255) & !255;
            if g.t.len() > 1 {
                crate::nvnlog!("pool #{} ({kind}, {n} entries): numbered from {b} in the host table", g.t.len());
            }
            b
        }
    };
    if base + n > g.end {
        g.end = base + n;
    }
    base
}
fn dpool_base(m: &Mutex<DpoolBases>, pool: usize) -> i32 {
    m.lock().unwrap().t.iter().find(|e| e.0 == pool).map(|e| e.1).unwrap_or(0)
}
/// The host table base of a texture (tex = true) or sampler descriptor pool: the replay's bound pools.
pub fn dpool_base_of(pool: *const dpool_t, tex: bool) -> i32 {
    if tex { dpool_base(&TEX_BASES, pool as usize) } else { dpool_base(&SAMP_BASES, pool as usize) }
}

nvn_fns! {
    // ---------- memory pools ----------
    fn nvnMemoryPoolBuilderSetDefaults(b: *mut NVNmemoryPoolBuilder) {
        let o = b as *mut pool_builder_t;
        (*o).mem = core::ptr::null_mut();
        (*o).size = 0;
        (*o).flags = mem_flags::CPU_NO_ACCESS | mem_flags::GPU_CACHED;
    }
    fn nvnMemoryPoolBuilderSetDevice(b: *mut NVNmemoryPoolBuilder, d: *mut NVNdevice) { (*(b as *mut pool_builder_t)).dev = d; }
    fn nvnMemoryPoolBuilderSetStorage(b: *mut NVNmemoryPoolBuilder, m: *mut u8, n: usize) {
        (*(b as *mut pool_builder_t)).mem = m;
        (*(b as *mut pool_builder_t)).size = n as u64;
    }
    fn nvnMemoryPoolBuilderSetFlags(b: *mut NVNmemoryPoolBuilder, f: i32) { (*(b as *mut pool_builder_t)).flags = f as u32; }
    fn nvnMemoryPoolBuilderGetDevice(b: *const NVNmemoryPoolBuilder) -> *mut NVNdevice { (*(b as *const pool_builder_t)).dev }
    fn nvnMemoryPoolBuilderGetMemory(b: *const NVNmemoryPoolBuilder) -> *mut u8 { (*(b as *const pool_builder_t)).mem }
    fn nvnMemoryPoolBuilderGetSize(b: *const NVNmemoryPoolBuilder) -> usize { (*(b as *const pool_builder_t)).size as usize }
    fn nvnMemoryPoolBuilderGetFlags(b: *const NVNmemoryPoolBuilder) -> u32 { (*(b as *const pool_builder_t)).flags }
    fn nvnMemoryPoolInitialize(p: *mut NVNmemoryPool, b: *const NVNmemoryPoolBuilder) -> NVNboolean {
        let s = &*(b as *const pool_builder_t);
        let o = p as *mut pool_t;
        *o = pool_t { magic: MAGIC_POOL, flags: s.flags, mem: s.mem, size: s.size, vp: core::ptr::null_mut(), vmode: 0, serial: super::new_serial() };
        if (*o).flags & mem_flags::VIRTUAL != 0 {
            (*o).mem = super::vpool::initialize(&mut *o) as *mut u8;
            return (!(*o).mem.is_null()) as u8;
        }
        if (*o).mem.is_null() {
            crate::log::red(&format!("MemoryPoolInitialize: non-virtual pool has no storage flags={:#x} size={:#x}", s.flags, s.size));
            return 1;
        }
        if (*o).flags & mem_flags::VIRTUAL == 0 {
            vk::mem::pool_add(&*o);
        }
        1
    }
    fn nvnMemoryPoolFinalize(p: *mut NVNmemoryPool) {
        let o = p as *mut pool_t;
        if (*o).flags & mem_flags::VIRTUAL != 0 {
            if (*o).vmode != super::vpool::ALIAS { super::vpool::finalize(&mut *o); }
        } else if !(*o).mem.is_null() {
            vk::mem::pool_del((*o).mem as u64);
        }
        (*o).magic = 0;
    }
    fn nvnMemoryPoolMap(p: *const NVNmemoryPool) -> *mut u8 { pool_cpu_map(&*(p as *const pool_t)) }
    fn nvnMemoryPoolFlushMappedRange(p: *const NVNmemoryPool, off: isize, n: usize) {
        let o = &*(p as *const pool_t);
        if !o.mem.is_null() { vk::mem::cpu_flushed(o.mem as u64 + off as u64, n as u64); }
    }
    fn nvnMemoryPoolInvalidateMappedRange(p: *const NVNmemoryPool, off: isize, n: usize) {
        let o = &*(p as *const pool_t);
        if !o.mem.is_null() { vk::mem::cpu_invalidate(o.mem as u64 + off as u64, n as u64); }
    }
    fn nvnMemoryPoolGetBufferAddress(p: *const NVNmemoryPool) -> u64 { (*(p as *const pool_t)).mem as u64 }
    fn nvnMemoryPoolGetSize(p: *const NVNmemoryPool) -> usize { (*(p as *const pool_t)).size as usize }
    fn nvnMemoryPoolGetFlags(p: *const NVNmemoryPool) -> u32 { (*(p as *const pool_t)).flags }
    fn nvnMemoryPoolMapVirtual(p: *mut NVNmemoryPool, n: i32, rq: *const u8) -> NVNboolean {
        let o = &mut *(p as *mut pool_t);
        if o.flags & mem_flags::VIRTUAL == 0 { crate::log::red_once("MapVirtual on a pool that is not virtual"); return 0; }
        let r = if n > 0 && !rq.is_null() { core::slice::from_raw_parts(rq as *const super::vpool::MapReq, n as usize) } else { &[] };
        super::vpool::map(o, r) as u8
    }

    // ---------- buffers ----------
    fn nvnBufferBuilderSetDefaults(b: *mut NVNbufferBuilder) {
        let o = b as *mut buf_builder_t;
        let d = (*o).dev;
        core::ptr::write_bytes(o as *mut u8, 0, core::mem::size_of::<buf_builder_t>());
        (*o).dev = d;
    }
    fn nvnBufferBuilderSetDevice(b: *mut NVNbufferBuilder, d: *mut NVNdevice) { (*(b as *mut buf_builder_t)).dev = d; }
    fn nvnBufferBuilderGetDevice(b: *const NVNbufferBuilder) -> *mut NVNdevice { (*(b as *const buf_builder_t)).dev }
    fn nvnBufferBuilderSetStorage(b: *mut NVNbufferBuilder, p: *mut NVNmemoryPool, off: isize, n: usize) {
        let o = b as *mut buf_builder_t;
        (*o).pool = p as *const pool_t;
        (*o).off = off as i64;
        (*o).size = n as u64;
    }
    fn nvnBufferBuilderGetMemoryPool(b: *const NVNbufferBuilder) -> *mut NVNmemoryPool { (*(b as *const buf_builder_t)).pool as *mut NVNmemoryPool }
    fn nvnBufferBuilderGetMemoryOffset(b: *const NVNbufferBuilder) -> isize { (*(b as *const buf_builder_t)).off as isize }
    fn nvnBufferBuilderGetSize(b: *const NVNbufferBuilder) -> usize { (*(b as *const buf_builder_t)).size as usize }
    fn nvnBufferInitialize(buf: *mut NVNbuffer, b: *const NVNbufferBuilder) -> NVNboolean {
        let bb = &*(b as *const buf_builder_t);
        core::ptr::write_bytes(buf as *mut u8, 0, core::mem::size_of::<buf_t>());
        if bb.pool.is_null() || (*bb.pool).flags & mem_flags::PHYSICAL != 0 {
            return 0;
        }
        *(buf as *mut buf_t) = buf_t { magic: MAGIC_BUF, pool: bb.pool, reserved: [0; 8], off: bb.off, size: bb.size };
        1
    }
    fn nvnBufferFinalize(buf: *mut NVNbuffer) { (*(buf as *mut buf_t)).magic = 0; }
    fn nvnBufferSetDebugLabel(buf: *mut NVNbuffer, s: *const u8) { let _ = (buf, s); }
    fn nvnBufferGetAddress(buf: *const NVNbuffer) -> u64 {
        let o = &*(buf as *const buf_t);
        (*o.pool).mem as u64 + o.off as u64
    }
    fn nvnBufferMap(buf: *const NVNbuffer) -> *mut u8 {
        let o = &*(buf as *const buf_t);
        let base = pool_cpu_map(&*o.pool);
        if base.is_null() { base } else { base.add(o.off as usize) }
    }
    fn nvnBufferGetSize(buf: *const NVNbuffer) -> usize { (*(buf as *const buf_t)).size as usize }
    fn nvnBufferGetMemoryPool(buf: *const NVNbuffer) -> *mut NVNmemoryPool { (*(buf as *const buf_t)).pool as *mut NVNmemoryPool }
    fn nvnBufferGetMemoryOffset(buf: *const NVNbuffer) -> isize { (*(buf as *const buf_t)).off as isize }
    fn nvnBufferFlushMappedRange(buf: *const NVNbuffer, off: isize, n: usize) {
        let o = &*(buf as *const buf_t);
        vk::mem::cpu_flushed((*o.pool).mem as u64 + o.off as u64 + off as u64, n as u64);
    }
    fn nvnBufferInvalidateMappedRange(buf: *const NVNbuffer, off: isize, n: usize) {
        let o = &*(buf as *const buf_t);
        vk::mem::cpu_invalidate((*o.pool).mem as u64 + o.off as u64 + off as u64, n as u64);
    }

    // ---------- descriptor pools ----------
    fn nvnTexturePoolInitialize(tp: *mut NVNtexturePool, p: *const NVNmemoryPool, off: isize, n: i32) -> NVNboolean {
        *(tp as *mut dpool_t) = dpool_t { pool: p as *const pool_t, off: off as i64, n };
        let base = dpool_add(&TEX_BASES, tp as usize, n, "texture");
        vk::Gpu::with(|g| g.tables.reserve_tex(base, n));
        1
    }
    fn nvnTexturePoolFinalize(tp: *mut NVNtexturePool) { let _ = tp; }
    fn nvnTexturePoolRegisterTexture(tp: *const NVNtexturePool, id: i32, t: *const NVNtexture, v: *const NVNtextureView) {
        let base = dpool_base(&TEX_BASES, tp as usize);
        let tex = *(t as *const tex_t);
        let view = if v.is_null() { None } else { Some(*(v as *const view_t)) };
        vk::Gpu::with(|g| g.reg_tex(base + id, &tex, view, false));
    }
    fn nvnTexturePoolRegisterImage(tp: *const NVNtexturePool, id: i32, t: *const NVNtexture, v: *const NVNtextureView) {
        let base = dpool_base(&TEX_BASES, tp as usize);
        let tex = *(t as *const tex_t);
        let view = if v.is_null() { None } else { Some(*(v as *const view_t)) };
        vk::Gpu::with(|g| g.reg_tex(base + id, &tex, view, true));
    }
    fn nvnTexturePoolGetMemoryPool(tp: *const NVNtexturePool) -> *const NVNmemoryPool { (*(tp as *const dpool_t)).pool as *const NVNmemoryPool }
    fn nvnTexturePoolGetMemoryOffset(tp: *const NVNtexturePool) -> isize { (*(tp as *const dpool_t)).off as isize }
    fn nvnTexturePoolGetSize(tp: *const NVNtexturePool) -> i32 { (*(tp as *const dpool_t)).n }
    fn nvnSamplerPoolInitialize(sp: *mut NVNsamplerPool, p: *const NVNmemoryPool, off: isize, n: i32) -> NVNboolean {
        *(sp as *mut dpool_t) = dpool_t { pool: p as *const pool_t, off: off as i64, n };
        let base = dpool_add(&SAMP_BASES, sp as usize, n, "sampler");
        vk::Gpu::with(|g| g.tables.reserve_samp(base, n));
        1
    }
    fn nvnSamplerPoolFinalize(sp: *mut NVNsamplerPool) { let _ = sp; }
    fn nvnSamplerPoolRegisterSampler(sp: *const NVNsamplerPool, id: i32, s: *const NVNsampler) {
        let base = dpool_base(&SAMP_BASES, sp as usize);
        let p = (*(s as *const samp_t)).p;
        vk::Gpu::with(|g| g.reg_samp(base + id, &p));
    }
    fn nvnSamplerPoolRegisterSamplerBuilder(sp: *const NVNsamplerPool, id: i32, b: *const NVNsamplerBuilder) {
        let base = dpool_base(&SAMP_BASES, sp as usize);
        let p = (*(b as *const samp_builder_t)).p;
        vk::Gpu::with(|g| g.reg_samp(base + id, &p));
    }
    fn nvnSamplerPoolGetMemoryPool(sp: *const NVNsamplerPool) -> *const NVNmemoryPool { (*(sp as *const dpool_t)).pool as *const NVNmemoryPool }
    fn nvnSamplerPoolGetMemoryOffset(sp: *const NVNsamplerPool) -> isize { (*(sp as *const dpool_t)).off as isize }
    fn nvnSamplerPoolGetSize(sp: *const NVNsamplerPool) -> i32 { (*(sp as *const dpool_t)).n }
}

//! Host functions that game patches call, keyed by the patched module's build id (the profile's `turbo.toml` cuts put
//! a `bl` into the patch page after `main`; `install` makes those addresses run the functions here).
//!
//! **Mario Kart World 1.8.0** (main build id d00e4f399e411d06, mkw-recomp's `runtime/core/boot/patches.c`, MIT, the
//! owner's): agl::utl::DynamicTextureAllocator (the render-buffer allocator, init main+0xeb1870) has one 285 MB pool
//! sized for the console's 2560x1440 targets and 3072 shadow map; 1.8.0's targets fill it and the out-of-memory path
//! hands the render thread a null block (mkw-recomp #255 / #210 / #305; lane X2's crash 4-5 s after the first frame).
//! The allocator grows only with a heap in its config (MKW passes none) and a free slot in its pool table (one slot).
//! The profile gives the table two slots and routes two calls here: the allocator's init (to remember the object) and
//! the shadow map's RenderBuffer creation (main+0xb72c0); at the first map this fills the second slot with a pool of
//! 128 MiB (photo mode at 1440 lines) plus one more 3072 map, from host memory under a game-class ExpHeap, taking
//! nothing from the game's own heaps.

use crate::guest;
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MKW_180_MAIN: &str = "d00e4f399e411d0613aa8d26b1c02fef";
/// Offsets in the patch page (mkw-recomp's: `SHADOW_POOL_HOST`, `SHADOW_HOST`).
const SHADOW_POOL_HOST: u64 = 0xf90;
const SHADOW_HOST: u64 = 0xfc0;
// MKW 1.8.0 main offsets (mkw-recomp runtime/core/boot/version.c)
const RB_CREATE: u64 = 0xb72c0;
const RB_ALLOC_INIT: u64 = 0xeb1870;
const RB_POOL_NEW: u64 = 0x3e3550;
const RB_POOL_INIT: u64 = 0xeb1e60;
const EXPHEAP_CREATE: u64 = 0xbeb4e0;
const PHOTO_MIB_1440: u64 = 128;

static MAIN: AtomicU64 = AtomicU64::new(0);
static RB_OBJ: AtomicU64 = AtomicU64::new(0);
static POOL_DONE: AtomicBool = AtomicBool::new(false);

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// After the modules are loaded: the host functions of the patches that apply to these modules.
pub fn install(loaded: &[nxrt::loader::Loaded]) {
    let Some(main) = loaded.iter().find(|m| m.name == "main") else { return };
    if !hex(&main.image.build_id).starts_with(MKW_180_MAIN) {
        return;
    }
    if std::env::var("NXRS_PATCH_MKW_RBPOOL").is_ok_and(|v| v == "0") {
        crate::log!("gamefix: Mario Kart World 1.8.0 render-buffer pool patch off (NXRS_PATCH_MKW_RBPOOL=0)");
        return;
    }
    MAIN.store(main.base, Ordering::Relaxed);
    let page = (main.base + main.size + 0xfff) & !0xfff;
    nxrt::hostfn::alias(page + SHADOW_POOL_HOST, "gamefix:mkw_shadow_pool", shadow_pool);
    nxrt::hostfn::alias(page + SHADOW_HOST, "gamefix:mkw_shadow_rb", shadow_rb);
    crate::log!("gamefix: Mario Kart World 1.8.0: the render-buffer allocator's second pool (mkw-recomp's shadow set) at patch page {page:#x}");
}

fn main_addr(off: u64) -> u64 {
    MAIN.load(Ordering::Relaxed) + off
}

fn map_bytes(size: u64) -> u64 {
    3 * 2 * size * size // 3 cascades of D16
}

/// The scene targets' lines: docked, 1.8.0 takes its 2160 layer (3840x2160 targets); handheld 1440 at most.
/// `NXRS_MKW_SCENE_LINES` overrides.
fn scene_lines() -> u64 {
    std::env::var("NXRS_MKW_SCENE_LINES").ok().and_then(|v| v.parse().ok())
        .unwrap_or(if crate::services::oe::opmode() == 1 { 2160 } else { 1440 })
}

/// mkw-recomp's 1.8.0 rule (second_pool_bytes) with the console's 3072 map: photo mode's 128 MiB at 1440 lines growing
/// with the pixels, a whole second map, and over 1440 lines the game's 250 MiB less its map growing with the pixels.
fn second_pool_bytes() -> u64 {
    let lines = scene_lines();
    let px = lines * lines;
    let base = 1440 * 1440;
    let mut b = (PHOTO_MIB_1440 << 20) * px / base + map_bytes(3072);
    if lines > 1440 {
        b += ((250u64 << 20) - map_bytes(3072)) * (px - base) / base;
    }
    (b + 0xffff) & !0xffff
}

/// Guest memory for a heap of the game's class: reserved and committed in the guest's 39-bit space below the kernel
/// region, an ExpHeap::create over it. 0 on failure.
unsafe fn host_heap(c: *mut Cpu, region_size: u64, text: &'static [u8]) -> u64 {
    let mut region = 0u64;
    let mut at = 0x60_0000_0000u64;
    while at + region_size < nxrt::mem::KERNEL_BASE {
        if nxrt::mem::reserve(at, region_size, false) {
            if nxrt::mem::commit(at, region_size) {
                region = at;
            }
            break;
        }
        at += 1 << 30;
    }
    if region == 0 {
        return 0;
    }
    let name = nxrt::kernel::get().space.kernel_alloc(8 + text.len() as u64, 8);
    core::ptr::copy_nonoverlapping(text.as_ptr(), (name + 8) as *mut u8, text.len());
    guest::w64(name, name + 8);
    guest::call(c, main_addr(EXPHEAP_CREATE), &[region, region_size, name, 1, 0]).unwrap_or(0)
}

/// The allocator's init (main+0xeb1870): remember the allocator, then run it. With
/// `NXRS_MKW_POOL_MODE=grow` (an experiment; mkw-recomp's second pool at the first shadow map is the default) the init's config gets a
/// growth heap at +0x30 (MKW passes 0) over 512 MiB of the host's, so the allocator's own path adds a pool of its
/// chunk size (config +0x10, 32 MiB) or the request's when the 285 MiB one is full (main+0xa7f14..0xa7fa4).
unsafe fn shadow_pool(c: *mut Cpu) {
    RB_OBJ.store((*c).x[0], Ordering::Relaxed);
    let (a0, a1) = ((*c).x[0], (*c).x[1]);
    let r = guest::call(c, main_addr(RB_ALLOC_INIT), &[a0, a1]);
    // the second pool at once (default): docked, 1.8.0's 2160-line targets fill the game's pool at the title, before
    // the first shadow map (x2-mkw31: two full-pool misses, then main+0x47ff40 frees a null block). The pool's memory
    // is the host's, so unlike mkw-recomp's at this point it takes nothing from the game's heaps.
    // `NXRS_MKW_POOL_MODE=map` waits for the first shadow map as mkw-recomp does.
    let mode = std::env::var("NXRS_MKW_POOL_MODE").unwrap_or_default();
    if mode == "grow" {
        // the allocator's own growth (main+0xa7f14): a heap at +0x2a78 (MKW's is 0), pools of +0x2a80 (32 MiB) or
        // the request into its table's free slot
        let heap = host_heap(c, 768 << 20, b"nxrs render-buffer growth heap\0");
        guest::w64(a0 + 0x2a78, heap);
        POOL_DONE.store(true, Ordering::Relaxed);
        crate::log!("gamefix: render-buffer allocator: growth heap {heap:#x} (768 MiB of the host's), chunk {:#x}", guest::r64(a0 + 0x2a80));
    }
    if mode.is_empty() && !POOL_DONE.swap(true, Ordering::Relaxed) {
        second_pool(c);
    }
    if let Some(r) = r {
        (*c).x[0] = r;
    }
}

/// The shadow map's RenderBuffer creation (main+0xb72c0): at the first square map of 512 or more, the second pool.
unsafe fn shadow_rb(c: *mut Cpu) {
    let a: [u64; 4] = [(*c).x[0], (*c).x[1], (*c).x[2], (*c).x[3]];
    let (w, h) = (guest::r32(a[0] + 0x68), guest::r32(a[0] + 0x88));
    if w == h && w >= 512 && !POOL_DONE.swap(true, Ordering::Relaxed) {
        second_pool(c);
    }
    if let Some(r) = guest::call(c, main_addr(RB_CREATE), &a) {
        (*c).x[0] = r;
    }
}

unsafe fn second_pool(c: *mut Cpu) {
    let rb = RB_OBJ.load(Ordering::Relaxed);
    if rb == 0 || (guest::r32(rb + 0x2a88) as i32) < 2 {
        crate::log!("[RED] gamefix: the render-buffer allocator has no second slot (is the profile's table patch built in?)");
        return;
    }
    let table = guest::r64(rb + 0x2a90);
    if guest::r64(table + 8) != 0 {
        crate::log!("gamefix: the render-buffer allocator's second slot is taken already");
        return;
    }
    let size = second_pool_bytes();
    let region_size = size + 0x20_0000;
    // inside the guest's 39-bit space (the GPU memory manager's regions came out empty for a pool at a 41-bit host
    // address), below the kernel region: mkw-recomp takes it from its runtime region the same way
    let mut region = 0u64;
    let mut at = 0x60_0000_0000u64;
    while at + region_size < nxrt::mem::KERNEL_BASE {
        if nxrt::mem::reserve(at, region_size, false) {
            if nxrt::mem::commit(at, region_size) {
                region = at;
            }
            break;
        }
        at += 1 << 30;
    }
    if region == 0 {
        crate::log!("[RED] gamefix: no guest address range for the render-buffer pool ({region_size:#x} bytes)");
        return;
    }
    // the heap's name: a pointer to the characters, as the game passes them
    let text = b"nxrs render-buffer pool\0";
    let name = nxrt::kernel::get().space.kernel_alloc(8 + text.len() as u64, 8);
    core::ptr::copy_nonoverlapping(text.as_ptr(), (name + 8) as *mut u8, text.len());
    guest::w64(name, name + 8);
    let heap = guest::call(c, main_addr(EXPHEAP_CREATE), &[region, region_size, name, 1, 0]).unwrap_or(0);
    let pool = if heap != 0 { guest::call(c, main_addr(RB_POOL_NEW), &[0x58, heap, 8]).unwrap_or(0) } else { 0 };
    if pool != 0 {
        // the block records a region has: the game's pool 0 regions' count (+0x48 of a region; [rb+0x11c0] is set
        // only later, 0 at the allocator's init, and a region without records "allocates" null blocks)
        let r0 = guest::r64(guest::r64(table) + 0x50);
        let count = (guest::r32(rb + 0x11c0) as u64).max(guest::r32(r0 + 0x48) as u64).max(0x100);
        crate::log!("gamefix: block records a region: {count:#x} (allocator {:#x}, pool 0 region {:#x})", guest::r32(rb + 0x11c0), guest::r32(r0 + 0x48));
        let flags = if guest::r8(rb + 0x2a68) & 0x80 != 0 { 0x104 } else { 4 };
        let kind = std::env::var("NXRS_MKW_POOL_KIND").ok().and_then(|v| v.parse().ok()).unwrap_or(0u64);
        let regions = std::env::var("NXRS_MKW_POOL_REGIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(7u64);
        guest::call(c, main_addr(RB_POOL_INIT), &[pool, kind, 1, regions, count, size, flags, heap]);
        guest::w64(table + 8, pool);
        let dump = |what: &str, a: u64| {
            let w: Vec<String> = (0..11).map(|i| format!("{:x}", guest::r64(a + 8 * i))).collect();
            crate::log!("gamefix: {what} {a:#x}: {}", w.join(" "));
            let regions = guest::r64(a + 0x50);
            for k in 0..(guest::r32(a + 0x48).min(2) as u64) {
                let r = regions + 0xe8 * k;
                let w: Vec<String> = (0..16).map(|i| format!("{:x}", guest::r64(r + 8 * i))).collect();
                crate::log!("gamefix:   region {k} {r:#x}: {}", w.join(" "));
            }
        };
        dump("pool 0", guest::r64(table));
        dump("pool 1", pool);
    }
    crate::log!(
        "gamefix: render-buffer allocator: {} second pool of {:#x} bytes on the host's heap {heap:#x} (region {region:#x})",
        if pool != 0 { "a" } else { "[RED] no" },
        size
    );
}

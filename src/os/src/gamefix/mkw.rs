//! Mario Kart World 1.8.0 (main build id d00e4f399e411d06): mkw-recomp's game layer (MIT): the host side of the
//! profile's patch sets (`src/profiles/games/0400C3F00006E000/recomp.txt`, from mkw-recomp's
//! `recomp/patches-mkw-180.txt`), from its `runtime/core/boot/patches.c`, `game.c`, `race_scene.c`, `fps_auto.c` and
//! `runtime/core/hacks/seed.c`. The guest addresses are its `version.c` 1.8.0 row.
//!
//! The patch page is the first 4 KiB page after main's image; each set's `bl` lands on one offset of it:
//!
//! | Offset | Set | Host function |
//! |---|---|---|
//! | 0xff0, 0xfe0 | zstd | `mkw_zstd.rs`: ZSTD_decompressMultiFrame, the loads' stream |
//! | 0xfd0 | output | data: the output size (two u32) the patched GraphicsSystem constructor loads |
//! | 0xfc0, 0xf90 | shadow | the shadow map's RenderBuffer creation, the render-buffer allocator's init |
//! | 0xfb0 | seed | GetSystemTick for the random generators' seeds |
//! | 0xfa0 | (frame rate) | the frame object's per-frame virtual (`NXRS_MKW_FPS`, a vtable slot, not a code site) |
//! | 0xf80, 0xf70, 0xf40, 0xf30, 0xf20, 0xdf0 | aspect, vertplus | `mkw_view.rs` |
//! | 0xf50 | camera | the camera service's event, with a stand-in service |
//! | 0xf10, 0xf00 | display | `mkw_view.rs` |
//! | 0xe00..0xef0 | (race scene) | the start methods of 14 sequence-node classes (vtable slots) |
//!
//! Settings (environment, all optional): `NXRS_MKW_SHADOW_SIZE` (the shadow map's size, 0 = the console's),
//! `NXRS_MKW_RB_EXTRA_MIB`, `NXRS_MKW_SCENE_LINES` (the scene targets' lines the second pool is sized for),
//! `NXRS_MKW_SEED` (a number: fixed random seeds), `NXRS_MKW_RESOLUTION` (lines of the output), `NXRS_MKW_ASPECT`
//! (`21:9`, `32:9`, ...), `NXRS_MKW_FPS` (`game`, `auto`, `60`, `30`, `20`, `15`). The sets themselves are switched by
//! their `NXRS_PATCH_MKW_*` variables (recomp.txt).

use super::{env_str, hex, set_on};
use crate::guest;
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

pub(super) const MKW_180_MAIN: &str = "d00e4f399e411d0613aa8d26b1c02fef";

// patch page offsets (mkw-recomp patches.c)
const OUTPUT_SIZE: u64 = 0xfd0;
const SHADOW_HOST: u64 = 0xfc0;
const SEED_HOST: u64 = 0xfb0;
const FRAME_HOST: u64 = 0xfa0;
const SHADOW_POOL_HOST: u64 = 0xf90;
const CAMERA_EVENT_HOST: u64 = 0xf50;
const RACE_HOST: u64 = 0xe00;

// MKW 1.8.0 main offsets (mkw-recomp runtime/core/boot/version.c)
const RB_CREATE: u64 = 0xb72c0;
const RB_ALLOC_INIT: u64 = 0xeb1870;
const RB_POOL_NEW: u64 = 0x3e3550;
const RB_POOL_INIT: u64 = 0xeb1e60;
const EXPHEAP_CREATE: u64 = 0xbeb4e0;
const RANDOM_CTOR_LR: u64 = 0xbe9b78;
const RANDOM_CTOR2_LR: u64 = 0x16a0ef0;
const FRAME_UPDATE: u64 = 0x69cbb0;
const FRAME_SLOT: u64 = 0x367aac0;
const GAME_CLOCK: u64 = 0x37c8438;
const CAMERA_SERVICE: u64 = 0x378ecb8;
const OUTPUT_LINES: u32 = 2160;
const PHOTO_MIB_1440: u64 = 128;

static MAIN: AtomicU64 = AtomicU64::new(0);
static PAGE: AtomicU64 = AtomicU64::new(0);
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub(super) fn main_addr(off: u64) -> u64 {
    MAIN.load(Ordering::Relaxed) + off
}

pub(super) fn now_ns() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// 60 Hz ticks since start (mkw-recomp's frame_step_ticks counts presents; the split-screen hold uses it as a clock)
pub(super) fn ticks60() -> u64 {
    now_ns() * 60 / 1_000_000_000
}

/// After the modules are loaded: the host functions of MKW 1.8.0's patch sets.
pub fn install(loaded: &[nxrt::loader::Loaded]) {
    let Some(main) = loaded.iter().find(|m| m.name == "main") else { return };
    if !hex(&main.image.build_id).starts_with(MKW_180_MAIN) {
        return;
    }
    START.get_or_init(Instant::now);
    MAIN.store(main.base, Ordering::Relaxed);
    let page = (main.base + main.size + 0xfff) & !0xfff;
    PAGE.store(page, Ordering::Relaxed);
    let al = |off: u64, name: &str, f: nxrt::hostfn::HostFn| nxrt::hostfn::alias(page + off, name, f);
    al(SHADOW_POOL_HOST, "gamefix:mkw_shadow_pool", shadow_pool);
    al(SHADOW_HOST, "gamefix:mkw_shadow_rb", shadow_rb);
    al(SEED_HOST, "gamefix:mkw_seed_tick", seed_tick_host);
    al(CAMERA_EVENT_HOST, "gamefix:mkw_camera_event", camera_event);
    super::mkw_zstd::install(page);
    super::mkw_view::install(page);
    let sets = [
        ("lan", "NXRS_PATCH_MKW_LAN", false),
        ("zstd", "NXRS_PATCH_MKW_ZSTD", true),
        ("output", "NXRS_PATCH_MKW_OUTPUT", false),
        ("camera", "NXRS_PATCH_MKW_CAMERA", true),
        ("aspect", "NXRS_PATCH_MKW_ASPECT", false),
        ("vertplus", "NXRS_PATCH_MKW_VERTPLUS", false),
        ("display", "NXRS_PATCH_MKW_DISPLAY", false),
        ("shadow", "NXRS_PATCH_MKW_SHADOW", true),
        ("seed", "NXRS_PATCH_MKW_SEED", false),
    ];
    let list: Vec<String> =
        sets.iter().map(|(n, e, d)| format!("{n} {}", if set_on(e, *d) { "on" } else { "off" })).collect();
    crate::log!("gamefix: Mario Kart World 1.8.0 (mkw-recomp's patch sets): {} (page {page:#x})", list.join(", "));
    shadow_settings();
    super::mkw_romfs::install();
    output_size(page);
    if set_on("NXRS_PATCH_MKW_SEED", false) && env_str("NXRS_MKW_SEED").is_empty() {
        crate::log!("[RED] gamefix: the seed set is on without NXRS_MKW_SEED: the random generators get seed 0's sequence");
    }
    frame_rate_init(page);
    race_scene_init(page);
}

// ---------------------------------------------------------------------------------------------------------------
// shadow: the render-buffer allocator's second pool (patches.c host_shadow_rb / host_shadow_pool / shadow_second_pool)
//
// agl::utl::DynamicTextureAllocator (init main+0xeb1870) has one pool sized for the console's 1440-line targets and
// 3072 shadow map; the shadow map shares it with the scene's targets. The table gets two slots (recomp.txt) and the
// second pool comes from host memory under a game-class ExpHeap (pead::ExpHeap::create, main+0xbeb4e0), taking
// nothing from the game's heaps.

static SHADOW_SIZE: AtomicU64 = AtomicU64::new(0);
static RB_EXTRA_MIB: AtomicU64 = AtomicU64::new(0);
static SCENE_LINES: AtomicU64 = AtomicU64::new(0);
static RB_OBJ: AtomicU64 = AtomicU64::new(0);
static POOL_DONE: AtomicBool = AtomicBool::new(false);
static POOL_OK: AtomicBool = AtomicBool::new(false);

fn env_u64(k: &str) -> Option<u64> {
    let v = env_str(k);
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    match v.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

fn shadow_settings() {
    SHADOW_SIZE.store(env_u64("NXRS_MKW_SHADOW_SIZE").unwrap_or(0), Ordering::Relaxed);
    RB_EXTRA_MIB.store(env_u64("NXRS_MKW_RB_EXTRA_MIB").unwrap_or(0), Ordering::Relaxed);
    // mkw-recomp: MKW_RESOLUTION's lines (romfs_res.c), 0 at "original" (the 1440 lines of the console's pool)
    let lines = env_u64("NXRS_MKW_SCENE_LINES").or_else(|| env_u64("NXRS_MKW_RESOLUTION")).unwrap_or(0);
    SCENE_LINES.store(lines, Ordering::Relaxed);
}

fn map_bytes(size: u64) -> u64 {
    3 * 2 * size * size // 3 cascades of D16
}

/// mkw-recomp's second_pool_bytes for 1.8.0: photo mode's 128 MiB at 1440 lines growing with the pixels, a whole
/// second map, a larger map's growth, and over 1440 lines the game's 250 MiB less its map growing with the pixels.
fn second_pool_bytes() -> u64 {
    let shadow = SHADOW_SIZE.load(Ordering::Relaxed);
    let scene = SCENE_LINES.load(Ordering::Relaxed);
    let mut b = RB_EXTRA_MIB.load(Ordering::Relaxed) << 20;
    if shadow > 3072 {
        b += map_bytes(shadow) - map_bytes(3072);
    }
    let lines = if scene != 0 { scene } else { 1440 };
    b += (PHOTO_MIB_1440 << 20) * lines * lines / (1440 * 1440) + map_bytes(if shadow != 0 { shadow } else { 3072 });
    if scene > 1440 {
        b += ((250u64 << 20) - map_bytes(3072)) * (scene * scene - 1440 * 1440) / (1440 * 1440);
    }
    (b + 0xffff) & !0xffff
}

/// The allocator's init (main+0xeb1870): remember the allocator, then run it.
unsafe fn shadow_pool(c: *mut Cpu) {
    RB_OBJ.store((*c).x[0], Ordering::Relaxed);
    let (a0, a1) = ((*c).x[0], (*c).x[1]);
    let r = guest::call(c, main_addr(RB_ALLOC_INIT), &[a0, a1]);
    // NXRS_MKW_POOL_MODE=early (lane X2's experiment): the second pool right after the init instead of at the first
    // shadow map (mkw-recomp's timing, the default)
    if env_str("NXRS_MKW_POOL_MODE") == "early" && !POOL_DONE.swap(true, Ordering::Relaxed) {
        POOL_OK.store(second_pool(c), Ordering::Relaxed);
    }
    if let Some(r) = r {
        (*c).x[0] = r;
    }
}

/// The shadow map's RenderBuffer creation (main+0xb72c0, x0 the buffer's descriptor: width +0x68, height +0x88): at
/// the first square map of 512 or more, the second pool; NXRS_MKW_SHADOW_SIZE resizes the map.
unsafe fn shadow_rb(c: *mut Cpu) {
    let a: [u64; 4] = [(*c).x[0], (*c).x[1], (*c).x[2], (*c).x[3]];
    let (w, h) = (guest::r32(a[0] + 0x68), guest::r32(a[0] + 0x88));
    let map = w == h && w >= 512;
    if map && !POOL_DONE.swap(true, Ordering::Relaxed) && second_pool_bytes() != 0 {
        POOL_OK.store(second_pool(c), Ordering::Relaxed);
    }
    let size = SHADOW_SIZE.load(Ordering::Relaxed) as u32;
    if size != 0 && map && (size <= 3072 || POOL_OK.load(Ordering::Relaxed)) {
        static LOGS: AtomicUsize = AtomicUsize::new(0);
        if w != size && LOGS.fetch_add(1, Ordering::Relaxed) < 4 {
            crate::log!("gamefix: shadow map {size}x{size} instead of {w}x{h} (NXRS_MKW_SHADOW_SIZE)");
        }
        guest::w32(a[0] + 0x68, size);
        guest::w32(a[0] + 0x88, size);
    }
    if let Some(r) = guest::call(c, main_addr(RB_CREATE), &a) {
        (*c).x[0] = r;
    }
}

/// Committed guest memory in the 39-bit space below the kernel region (the GPU memory manager wants a guest
/// address), as mkw-recomp's mem_runtime_alloc gives one from the runtime's region. 0 when there is none.
fn host_region(size: u64) -> u64 {
    let mut at = 0x60_0000_0000u64;
    while at + size < nxrt::mem::KERNEL_BASE {
        if nxrt::mem::reserve(at, size, false) {
            return if nxrt::mem::commit(at, size) { at } else { 0 };
        }
        at += 1 << 30;
    }
    0
}

/// The second pool as the allocator makes one, from a heap of the game's class over host memory
/// (ExpHeap::create(start, size, name, lock, 0); operator new(0x58, heap, 8); the pool init with the first pool's
/// record count, the allocator's flags and that heap).
unsafe fn second_pool(c: *mut Cpu) -> bool {
    let saved: [u64; 4] = [(*c).x[0], (*c).x[1], (*c).x[2], (*c).x[3]];
    let rb = RB_OBJ.load(Ordering::Relaxed);
    if rb == 0 || (guest::r32(rb + 0x2a88) as i32) < 2 || guest::r64(guest::r64(rb + 0x2a90) + 8) != 0 {
        crate::log!("[RED] gamefix: the render-buffer allocator has no free second slot; no second pool");
        return false;
    }
    let table = guest::r64(rb + 0x2a90);
    let size = second_pool_bytes();
    let region_size = size + 0x20_0000; // the heap's header and the pool's alignment
    let region = host_region(region_size);
    let mut heap = 0;
    if region != 0 {
        let text = b"nxrs render-buffer pool\0";
        let name = nxrt::kernel::get().space.kernel_alloc(8 + text.len() as u64, 8);
        core::ptr::copy_nonoverlapping(text.as_ptr(), (name + 8) as *mut u8, text.len());
        guest::w64(name, name + 8);
        heap = guest::call(c, main_addr(EXPHEAP_CREATE), &[region, region_size, name, 1, 0]).unwrap_or(0);
    }
    let pool = if heap != 0 { guest::call(c, main_addr(RB_POOL_NEW), &[0x58, heap, 8]).unwrap_or(0) } else { 0 };
    if pool != 0 {
        // mkw-recomp passes the allocator's record count ([rb+0x11c0]); it is 0 while the init has not filled it
        // (NXRS_MKW_POOL_MODE=early), where the first pool's region count stands in (a region without records
        // hands out null blocks)
        let mut count = guest::r32(rb + 0x11c0) as u64;
        if count == 0 {
            count = (guest::r32(guest::r64(guest::r64(table) + 0x50) + 0x48) as u64).max(0x100);
        }
        let flags = if guest::r8(rb + 0x2a68) & 0x80 != 0 { 0x104 } else { 4 };
        guest::call(c, main_addr(RB_POOL_INIT), &[pool, 0, 1, 7, count, size, flags, heap]);
        guest::w64(table + 8, pool);
    }
    crate::log!(
        "gamefix: render-buffer allocator: {} second pool of {size:#x} bytes on the host's heap {heap:#x} (region {region:#x}; shadow map {}, scene {} lines, NXRS_MKW_RB_EXTRA_MIB {})",
        if pool != 0 { "a" } else { "[RED] no" },
        SHADOW_SIZE.load(Ordering::Relaxed),
        SCENE_LINES.load(Ordering::Relaxed),
        RB_EXTRA_MIB.load(Ordering::Relaxed)
    );
    for (i, v) in saved.iter().enumerate() {
        (*c).x[i] = *v;
    }
    pool != 0
}

// ---------------------------------------------------------------------------------------------------------------
// seed (core/hacks/seed.c): NXRS_MKW_SEED in place of the clock the game seeds its random generators with

const SEED_SITES: usize = 256;
static SEED_LR: [AtomicU64; SEED_SITES] = [const { AtomicU64::new(0) }; SEED_SITES];
static SEED_N: [AtomicU64; SEED_SITES] = [const { AtomicU64::new(0) }; SEED_SITES];

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// The k-th seed asked for from one call site: a count per site (several threads seed in an order that changes).
fn seed_tick(lr: u64) -> u64 {
    let mut i = 0;
    while i < SEED_SITES {
        let have = SEED_LR[i].load(Ordering::Acquire);
        if have == lr {
            break;
        }
        if have == 0 {
            match SEED_LR[i].compare_exchange(0, lr, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    crate::log!("gamefix: the game's random generator seeded from NXRS_MKW_SEED={} at lr {lr:#x}", env_str("NXRS_MKW_SEED"));
                    break;
                }
                Err(now) if now == lr => break,
                Err(_) => {}
            }
        }
        i += 1;
    }
    let n = if i < SEED_SITES { SEED_N[i].fetch_add(1, Ordering::Relaxed) } else { 0 };
    let base = env_u64("NXRS_MKW_SEED").unwrap_or(0);
    mix(base ^ mix(lr) ^ (n + 1).wrapping_mul(0x9e3779b97f4a7c15)) >> 8 // a 56-bit tick
}

/// In place of the bl to nn::os::GetSystemTick before a generator's seeding: the two sead::Random constructors are
/// told apart by their creator, whose return address their prologue saved at [sp+8] and [sp].
unsafe fn seed_tick_host(c: *mut Cpu) {
    let mut lr = (*c).x[30];
    if lr == main_addr(RANDOM_CTOR_LR) {
        lr ^= guest::r64((*c).sp + 8) << 1;
    } else if lr == main_addr(RANDOM_CTOR2_LR) {
        lr ^= guest::r64((*c).sp) << 1;
    }
    (*c).x[0] = seed_tick(lr);
}

// ---------------------------------------------------------------------------------------------------------------
// camera (patches.c host_camera_event): the console with no camera attached. main+0xcdebc0 (which opens the camera
// service) does not run; in place of main+0xcdc560 the event is a real one that never fires, and the global the
// service object lives in gets a stand-in whose every request fails as a camera that is not there: 0x7c4 for slot
// 0xd8 (asked at boot and passed over), 0x5c4 for the rest.

const CAMERA_SLOTS: u64 = 64;

unsafe fn camera_absent(c: *mut Cpu) {
    (*c).x[0] = 0x7c4;
}

unsafe fn camera_unavailable(c: *mut Cpu) {
    (*c).x[0] = 0x5c4;
}

unsafe fn camera_event(c: *mut Cpu) {
    let Some(create) = crate::hle::sym("_ZN2nn2os17CreateSystemEventEPNS0_15SystemEventTypeENS0_14EventClearModeEb") else {
        nxrt::rt::fatal("gamefix: nn::os::CreateSystemEvent not found (Mario Kart World's camera set)");
    };
    let ev = (*c).x[0];
    let at = main_addr(CAMERA_SERVICE);
    if guest::r64(at) == 0 {
        let absent = crate::hle::addr("gamefix:mkw_camera_absent", camera_absent);
        let unavailable = crate::hle::addr("gamefix:mkw_camera_unavailable", camera_unavailable);
        let obj = nxrt::kernel::get().space.kernel_alloc(16 + 8 * CAMERA_SLOTS, 16);
        let vt = obj + 16;
        for i in 0..CAMERA_SLOTS {
            guest::w64(vt + 8 * i, if i == 0xd8 / 8 { absent } else { unavailable });
        }
        guest::w64(obj, vt);
        guest::w64(at, obj);
        crate::log!("gamefix: no camera: the camera service is a stand-in whose requests fail (NXRS_PATCH_MKW_CAMERA)");
    }
    guest::call(c, create, &[ev, 0, 0]);
}

// ---------------------------------------------------------------------------------------------------------------
// output (patches.c output_size): with NXRS_MKW_RESOLUTION, that many lines (at 16:9, or at NXRS_MKW_ASPECT) in
// place of the 3840x2160 output GraphicsSystem's constructor fixes; the set reads it from the patch page. 1.8.0 draws
// its last pass at the screen render's size, so an output of fewer lines than that cuts the picture: its output keeps
// at least those lines (mkw-recomp: final_at_screen).

static OUTPUT_WH: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

pub(super) fn output_wh() -> (u32, u32) {
    (OUTPUT_WH[0].load(Ordering::Relaxed) as u32, OUTPUT_WH[1].load(Ordering::Relaxed) as u32)
}

fn output_size(page: u64) {
    let native = OUTPUT_LINES;
    OUTPUT_WH[0].store((native * 16 / 9) as u64, Ordering::Relaxed);
    OUTPUT_WH[1].store(native as u64, Ordering::Relaxed);
    let on = set_on("NXRS_PATCH_MKW_OUTPUT", false);
    let lines = env_u64("NXRS_MKW_RESOLUTION").unwrap_or(0) as u32;
    let (aspect_out, scale) = super::mkw_view::aspect();
    let display = set_on("NXRS_PATCH_MKW_DISPLAY", false);
    // 1.8.0 (final_at_screen): "original" keeps the game's output; a wider aspect takes 1440 lines
    let lines = if lines == 0 && aspect_out > 0.0 && !(scale < 1.0) { 1440 } else { lines };
    if !on {
        if lines != 0 {
            crate::log!("[RED] gamefix: NXRS_MKW_RESOLUTION / ASPECT need the output set (NXRS_PATCH_MKW_OUTPUT=1); the output stays {}x{native}", native * 16 / 9);
        }
        return;
    }
    if lines == 0 {
        crate::log!("[RED] gamefix: the output set is on without NXRS_MKW_RESOLUTION or a wider NXRS_MKW_ASPECT: the patched constructor would read 0x0");
        return;
    }
    let tall_at_screen = aspect_out > 0.0 && scale < 1.0;
    let wide = if aspect_out > 0.0 && !tall_at_screen { ((lines as f32 * aspect_out + 0.5) as u32 + 1) & !1 } else { 0 };
    let mut wh = [if wide != 0 { wide } else { lines * 16 / 9 }, lines];
    if aspect_out > 0.0 && scale < 1.0 && display {
        // aspect_tall_target: keep the width, add rows
        wh[0] = lines * 16 / 9;
        wh[1] = ((wh[0] as f32 / aspect_out + 0.5) as u32 + 1) & !1;
    }
    OUTPUT_WH[0].store(wh[0] as u64, Ordering::Relaxed);
    OUTPUT_WH[1].store(wh[1] as u64, Ordering::Relaxed);
    // the page holds the output size: committed only for it (its hooks are host aliases, no memory). Committed for
    // every run, the page after main changed the module region's memory state and main's init aborted (Result 0xe801,
    // main+0xf42348, run K-mkw1-port)
    if !nxrt::mem::commit(page, 0x1000) && !(nxrt::mem::reserve(page, 0x1000, false) && nxrt::mem::commit(page, 0x1000)) {
        crate::log!("[RED] gamefix: cannot commit Mario Kart World's patch page {page:#x}: the output stays the game's");
        return;
    }
    guest::w32(page + OUTPUT_SIZE, wh[0]);
    guest::w32(page + OUTPUT_SIZE + 4, wh[1]);
    crate::log!(
        "gamefix: game output {}x{} instead of {}x{native} (NXRS_MKW_RESOLUTION {lines}, NXRS_MKW_ASPECT {})",
        wh[0],
        wh[1],
        native * 16 / 9,
        env_str("NXRS_MKW_ASPECT")
    );
}

// ---------------------------------------------------------------------------------------------------------------
// frame rate (game.c, fps_auto.c): NXRS_MKW_FPS. The game picks its present interval each frame in its frame
// object's per-frame virtual (main+0x69cbb0, vtable slot main+0x367aac0) from the time it measured for the frame;
// [obj+0x210] = 1 or 2 is its own fixed interval, which skips that choice, and any other non-zero value keeps [+0x214]
// as it is (20 and 15 fps). Unset or `game`: the game's own choice (the slot is left alone).

const FRAME_FIXED: u64 = 0x210;
const FRAME_INTERVAL: u64 = 0x214;
const FRAME_MEASURED: u64 = 0x128;
const OUT_OF_RACE_MAX: i32 = 2;
const TICK_NS: f64 = 32.0; // 31.25 MHz

fn fixed_interval(fps: &str) -> i32 {
    match fps {
        "60" => 1,
        "30" => 2,
        "20" => 3,
        "15" => 4,
        _ => 0,
    }
}

const FPS_AUTO_HIST: usize = 120;
const FPS_AUTO_MAX: i32 = 4;
const PERIOD_MS: f64 = 1000.0 / 60.0;
const UP_FRAMES: usize = 8;
const DOWN_MARGIN: f64 = 0.85;
const DOWN_SECONDS: u32 = 2;
const HOLD_MIN_S: u32 = 5;
const HOLD_MAX_S: u32 = 60;
const PINGPONG_S: u32 = 10;
const UP_MARGIN: f64 = 1.08;
const STALL_FACTOR: f64 = 2.0;
const STALL_MAX_S: f64 = 20.0;

/// fps_auto.c's controller: up as soon as the median of the last 8 frames is 8 % past the interval, down only when
/// the median of the last two seconds fits a lower one with 15 % to spare; a stall (frames past 133 ms after frames
/// that fitted) moves nothing for up to 20 s.
struct FpsAuto {
    hist: [f64; FPS_AUTO_HIST],
    real: [f64; UP_FRAMES],
    head: usize,
    n: usize,
    n_real: usize,
    since: u32,
    hold: u32,
    hold_s: u32,
    iv: i32,
    downed: bool,
    fits: bool,
    stall_ms: f64,
}

impl FpsAuto {
    const fn new() -> Self {
        FpsAuto { hist: [0.0; FPS_AUTO_HIST], real: [0.0; UP_FRAMES], head: 0, n: 0, n_real: 0, since: 0, hold: 0, hold_s: 0, iv: 1, downed: false, fits: false, stall_ms: 0.0 }
    }
    fn median(&self, count: usize) -> f64 {
        let n = count.min(self.n);
        let mut v: Vec<f64> = (0..n).map(|i| self.hist[(self.head + FPS_AUTO_HIST - 1 - i) % FPS_AUTO_HIST]).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        if n > 0 { v[n / 2] } else { 0.0 }
    }
    fn median_real(&self) -> f64 {
        let mut v = self.real;
        v.sort_by(|a, b| a.total_cmp(b));
        v[UP_FRAMES / 2]
    }
    fn fitting(ms: f64, margin: f64) -> i32 {
        (1..FPS_AUTO_MAX).find(|&iv| ms <= iv as f64 * PERIOD_MS * margin).unwrap_or(FPS_AUTO_MAX)
    }
    fn step(&mut self, measured_ms: f64, real_ms: f64) -> i32 {
        self.hist[self.head] = measured_ms;
        self.head = (self.head + 1) % FPS_AUTO_HIST;
        self.real[self.n_real % UP_FRAMES] = real_ms;
        self.n_real += 1;
        if self.n < FPS_AUTO_HIST {
            self.n += 1;
        }
        self.since += 1;
        if self.hold > 0 {
            self.hold -= 1;
        }
        if self.hold_s == 0 {
            self.hold_s = HOLD_MIN_S;
        }
        if self.since as usize >= UP_FRAMES && self.n >= UP_FRAMES {
            let m = self.median(UP_FRAMES);
            let r = if self.n_real >= UP_FRAMES { self.median_real() } else { 0.0 };
            let v = m.max(r);
            let stalled = self.fits && v > FPS_AUTO_MAX as f64 * PERIOD_MS * STALL_FACTOR;
            self.stall_ms = if stalled { self.stall_ms + real_ms.max(measured_ms) } else { 0.0 };
            let up = if stalled && self.stall_ms < STALL_MAX_S * 1000.0 { self.iv } else { Self::fitting(v, UP_MARGIN) };
            if !stalled {
                self.fits = up <= self.iv;
            }
            if up > self.iv {
                let undid_down = self.downed && self.since < PINGPONG_S * 60 / self.iv as u32;
                self.hold_s = if undid_down { (self.hold_s * 2).min(HOLD_MAX_S) } else { HOLD_MIN_S };
                self.iv = up;
                self.since = 0;
                self.downed = false;
                self.hold = self.hold_s * 60 / self.iv as u32;
                return self.iv;
            }
        }
        let window = DOWN_SECONDS * 60 / self.iv as u32;
        if self.iv > 1 && self.hold == 0 && self.since >= window && self.n >= window as usize && Self::fitting(self.median(window as usize), DOWN_MARGIN) < self.iv {
            self.iv -= 1;
            self.since = 0;
            self.downed = true;
        }
        self.iv
    }
}

static AUTO: Mutex<(FpsAuto, u64)> = Mutex::new((FpsAuto::new(), 0));
static FPS_SAVED: AtomicI32 = AtomicI32::new(-1);
static WAS_CAPPED: AtomicBool = AtomicBool::new(false);
static WAS_MOVIE: AtomicBool = AtomicBool::new(false);
static SPEED: Mutex<(u64, u32, u32)> = Mutex::new((0, 0, 0));

fn auto_want(obj: u64) -> i32 {
    let mut g = AUTO.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = now_ns();
    let ms = guest::r64(obj + FRAME_MEASURED) as i64 as f64 * TICK_NS / 1e6;
    let real = if g.1 != 0 { (now - g.1) as f64 / 1e6 } else { 0.0 };
    g.1 = now;
    let prev = g.0.iv;
    let iv = g.0.step(ms, real);
    if iv != prev {
        crate::log!("gamefix: auto interval {prev} -> {iv} (frame {ms:.1} ms, real {real:.1} ms)");
    }
    // the game's own choice (1 or 2) while 30 fps fits; 3 or 4 only in a race
    if iv > OUT_OF_RACE_MAX && race_on() { iv } else { 0 }
}

/// Game seconds per real second every 5 s (game.c speed_note): below 1 the game runs in slow motion.
fn speed_note() {
    let clock = guest::r32(main_addr(GAME_CLOCK));
    let now = now_ns();
    let mut s = SPEED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if s.0 == 0 {
        *s = (now, clock, 0);
        return;
    }
    s.2 += 1;
    if now - s.0 < 5_000_000_000 {
        return;
    }
    let real = (now - s.0) as f64 / 1e9;
    crate::log!("gamefix: game speed {:.2}x over {real:.1} s ({} frames, NXRS_MKW_FPS {})", clock.wrapping_sub(s.1) as f64 / 60.0 / real, s.2, env_str("NXRS_MKW_FPS"));
    *s = (now, clock, 0);
}

/// The game thread, once per frame: NXRS_MKW_FPS into the fixed interval, then the game's own function.
unsafe fn frame_update(c: *mut Cpu) {
    let obj = (*c).x[0];
    let fps = env_str("NXRS_MKW_FPS");
    let movie = crate::services::movie::playing();
    if movie != WAS_MOVIE.swap(movie, Ordering::Relaxed) {
        crate::log!("gamefix: {}", if movie { "a movie plays: the game picks the interval" } else { "movie over" });
    }
    let mut want = if movie || fps == "game" {
        0
    } else if fps == "auto" {
        auto_want(obj)
    } else {
        fixed_interval(&fps)
    };
    // 20 and 15 only in a race: menus and loads go at 30 at most, as on the console
    let capped = want > OUT_OF_RACE_MAX && !race_on();
    if capped != WAS_CAPPED.swap(capped, Ordering::Relaxed) {
        crate::log!("gamefix: interval {want} {}", if capped { "capped at 2 (out of a race)" } else { "no longer capped" });
    }
    if capped {
        want = OUT_OF_RACE_MAX;
    }
    if want != 0 {
        if FPS_SAVED.load(Ordering::Relaxed) < 0 {
            let had = guest::r32(obj + FRAME_FIXED) as i32;
            FPS_SAVED.store(had, Ordering::Relaxed);
            crate::log!("gamefix: present interval fixed at {want} (NXRS_MKW_FPS {fps}; the game's value was {had})");
        }
        guest::w32(obj + FRAME_FIXED, want as u32);
        guest::w32(obj + FRAME_INTERVAL, want as u32);
    } else {
        let saved = FPS_SAVED.swap(-1, Ordering::Relaxed);
        if saved >= 0 {
            guest::w32(obj + FRAME_FIXED, saved as u32);
            crate::log!("gamefix: present interval chosen by the game again (NXRS_MKW_FPS {fps})");
        }
    }
    guest::call(c, main_addr(FRAME_UPDATE), &[obj]);
    speed_note();
}

/// After main is loaded: the frame object's vtable slot calls frame_update instead (only with NXRS_MKW_FPS set).
fn frame_rate_init(page: u64) {
    let fps = env_str("NXRS_MKW_FPS");
    if fps.is_empty() || fps == "game" {
        return;
    }
    if fps != "auto" && fixed_interval(&fps) == 0 {
        crate::log!("[RED] gamefix: NXRS_MKW_FPS={fps} is not game, auto, 60, 30, 20 or 15");
        return;
    }
    let slot = main_addr(FRAME_SLOT);
    if guest::r64(slot) != main_addr(FRAME_UPDATE) {
        crate::log!("[RED] gamefix: vtable slot {slot:#x} holds {:#x}, not the frame update: NXRS_MKW_FPS has no effect", guest::r64(slot));
        return;
    }
    nxrt::hostfn::alias(page + FRAME_HOST, "gamefix:mkw_frame_update", frame_update);
    guest::w64(slot, page + FRAME_HOST);
    crate::log!("gamefix: the frame update goes through NXRS_MKW_FPS={fps}");
}

// ---------------------------------------------------------------------------------------------------------------
// race scene (race_scene.c): whether the game is in a race, from the start method (vtable slot +10 after the name
// getter) of 14 sequence-node classes; only NXRS_MKW_FPS's 20 and 15 read it.

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Arm,
    Start,
    Leave,
    Chat,
    FreeArm,
}

const RACE_EVENTS: [(&str, u64, Role); 14] = [
    ("ReadyRace", 0x353f5c0, Role::Arm),
    ("StartIntroMovie", 0x35448c0, Role::Arm),
    ("WaitIntroMovie", 0x354a5d8, Role::Arm),
    ("IsSequenceFlag", 0x353b3a0, Role::Start),
    ("RaceFinishFaderOpenCheck", 0x353f180, Role::Leave),
    ("GrandprixEndMovie", 0x3539c68, Role::Leave),
    ("GrandprixListup", 0x3539db8, Role::Leave),
    ("TitleMenuMain", 0x3547698, Role::Leave),
    ("VSCourseSelect", 0x35492e8, Role::Leave),
    ("MenuTimeAttackMain", 0x353ce78, Role::Leave),
    ("ReturnToTitleMenu", 0x35409c8, Role::Leave),
    ("PartsSelectEx", 0x353df10, Role::Leave),
    ("ChatUIReady", 0x3534d90, Role::Chat),
    ("ReadyFreeRun", 0x353f420, Role::FreeArm),
];
const SLOT_ENTER: u64 = 10;

static IN_RACE: AtomicBool = AtomicBool::new(false);
static ARMED: AtomicBool = AtomicBool::new(false);
static FREE_ARMED: AtomicBool = AtomicBool::new(false);
static RACE_ORIG: [AtomicU64; 14] = [const { AtomicU64::new(0) }; 14];

pub(super) fn race_on() -> bool {
    IN_RACE.load(Ordering::Relaxed)
}

unsafe fn node_start(c: *mut Cpu, k: usize) {
    let (name, _, role) = RACE_EVENTS[k];
    let was = IN_RACE.load(Ordering::Relaxed);
    match role {
        Role::Arm => {
            ARMED.store(true, Ordering::Relaxed);
            IN_RACE.store(false, Ordering::Relaxed);
        }
        Role::Start if ARMED.load(Ordering::Relaxed) => {
            ARMED.store(false, Ordering::Relaxed);
            IN_RACE.store(true, Ordering::Relaxed);
        }
        Role::Start => {}
        Role::Leave => {
            ARMED.store(false, Ordering::Relaxed);
            FREE_ARMED.store(false, Ordering::Relaxed);
            IN_RACE.store(false, Ordering::Relaxed);
        }
        Role::FreeArm => {
            FREE_ARMED.store(true, Ordering::Relaxed);
            IN_RACE.store(false, Ordering::Relaxed);
        }
        // ChatUIReady starts at a race's finish and when free roam begins (right after ReadyFreeRun)
        Role::Chat => {
            IN_RACE.store(FREE_ARMED.swap(false, Ordering::Relaxed), Ordering::Relaxed);
            ARMED.store(false, Ordering::Relaxed);
        }
    }
    let now = IN_RACE.load(Ordering::Relaxed);
    if now != was {
        crate::log!("gamefix: race: {} at the start of {name}", if now { "in a race" } else { "out of the race" });
    }
    let x = (*c).x;
    let a: Vec<u64> = x[..8].to_vec();
    guest::call(c, RACE_ORIG[k].load(Ordering::Relaxed), &a);
}

macro_rules! race_hosts {
    ($($k:literal),*) => {
        const RACE_HOSTS: [nxrt::hostfn::HostFn; 14] = [$({ unsafe fn h(c: *mut Cpu) { node_start(c, $k) } h }),*];
    };
}
race_hosts!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13);

/// With NXRS_MKW_FPS's 20 or 15 (or auto, which reaches them): each class's start slot calls node_start first.
fn race_scene_init(page: u64) {
    let fps = env_str("NXRS_MKW_FPS");
    if !matches!(fps.as_str(), "auto" | "20" | "15") {
        return;
    }
    for (k, (name, slot, _)) in RACE_EVENTS.iter().enumerate() {
        let at = main_addr(*slot) + 8 * SLOT_ENTER;
        RACE_ORIG[k].store(guest::r64(at), Ordering::Relaxed);
        let host = page + RACE_HOST + 0x10 * k as u64;
        nxrt::hostfn::alias(host, &format!("gamefix:mkw_race_{name}"), RACE_HOSTS[k]);
        guest::w64(at, host);
    }
    crate::log!("gamefix: race scene: 14 sequence nodes watched (NXRS_MKW_FPS {fps}: 20 and 15 only in a race)");
}

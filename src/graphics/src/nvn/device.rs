//! Device: builder, initialization (which brings up the Vulkan back end), device properties, handles, modes.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// NVN2 (the Switch 2's SDK): the device API, GOB order and counter timestamp units follow it. Set by lane A from the
/// game's sdk module before nvnDeviceInitialize.
pub static SWITCH2: AtomicBool = AtomicBool::new(false);
/// The SDK major version of the game (SDK 10 and older report API 53.313). 0 = unknown (55.15).
pub static SDK_MAJOR: AtomicI32 = AtomicI32::new(0);
/// Device window origin and depth mode (the replay reads them; the queue captures the origin at its creation).
pub static ORIGIN_UPPER_LEFT: AtomicBool = AtomicBool::new(false);
pub static DEPTH_ZERO: AtomicBool = AtomicBool::new(false);

/// Device properties measured on Tegra X1 (items 0 .. 0x68).
pub const DEVINFO: [i32; 0x69] = [
    55, 15, 14, 65536, 256, 8, 16, 4, 16, 32, 16, 4, 4, 4, 16, 32, 32, 256, 256, 4, 8, 512, 256, 2,
    256, 4096, 131072, 16, 1536, 1024, 64, 1536, 65535, 65535, 65535, 8, 1048576, 4096, 16, 4096,
    1, 1, 1, 262144, 15, 2048, 1, 1, 14, 5, 1, 8, 8, 4, 32, 128, 32, 128, 65536, 1, 4, 16384,
    134217728, 2048, 16384, 16384, 1, 1, 1, 1, 5, 1, 1, 1, 1, 1, 1, 16, 1024, 32, 4096, 65536,
    65536, 256, 16384, 262144, 4096, 1, 4, 2, 0, 4096, 16384, 4096, 128, 32, 65535, 1, 32, 256, 16,
    65535, 65535, 256, 16383,
];

/// An NVN2 item the driver reads from the GPU's own description at run time (not knowable from the binary).
const RT: i32 = -1;
/// The Switch 2's answers (NVN2 compiled into the game's sdk answers from constants, read from
/// Donkey Kong Bananza's sdk 20.5.19 and SDK 21.4.4's). Ampere differs from the X1 in 32 vertex bindings, 1,024
/// compute threads, an unbounded dispatch width, 16 texture levels, 16 KB scratch granularity, 2 KB shader code
/// padding, 32 separate sampler bindings, and items up to 0xA9. `RT` items take the X1 table's value where it has one
/// (0x62: 32, as nxnative) and answer 0 (with a red line) past it. 0x06 answers 16 vertex bindings: the host keeps 16.
pub const DEVINFO2: [i32; 0xAA] = [
    231, 39, 14, 65536, 256, 8, 16, 4, 16, 32, 16, 4, 4, 4, 32,
    32, // 0x00 (0x06: Ampere has 32 vertex bindings, the host keeps 16 as all its tables do; nxnative too)
    32, 256, 256, 4, 8, 512, 256, RT, RT, 4096, 16384, 16, 1024, 1024, 64, 1024, // 0x10
    2147483647, 65535, 65535, 8, 1048576, 4096, 16, 4096, 1, 1, 1, RT, 16, RT, 201,
    201, // 0x20
    20, 0, 1, 8, 8, 4, 32, RT, RT, 128, RT, 1, 4, RT, RT, RT, // 0x30
    RT, RT, 1, 1, 1, 1, 5, 1, 1, 1, 1, 1, 1, 16, 2048, 32, // 0x40
    4096, 65536, 65536, 256, 16384, 262144, 4096, 1, 4, 2, 0, 4096, 16384, 4096, 128,
    32, // 0x50
    65535, 1, RT, 256, 16, 65535, 65535, 256, 1, 16, 16, 16, 1, 1, 1, 256, // 0x60
    0, 16, 8, 16, 32, 31, 536870912, 16777216, 16777216, 1073741824, 32, 1, 64, RT, 1,
    2147483647, // 0x70
    32, 32, 4194303, 512, 256, 32, 16383, RT, 8, 0, 0, 4, 256, 16, 0, 1, // 0x80
    31, 0, 64, 1, 58368, 24576, 0, 256, 262144, RT, RT, 0, 201, 14, 201, 20, // 0x90
    163840, 8, 4, 256, 256, 4, 65536, 201, 5, 1, // 0xA0
];

/// The constants of NVN2's own nvnDeviceGetInteger, read from the game's sdk (`set_sdk_image`): its API and GPU code
/// versions move with the SDK (20.5.19: API 231.30; 21.4.4: 231.39) and games check them.
static NVN2_ITEMS: std::sync::OnceLock<Vec<Option<i32>>> = std::sync::OnceLock::new();

/// The game's sdk module as loaded (text and data at their offsets from the module's start): the Switch 2's device
/// answers come from its driver's switch where it has a constant case. os calls this once
/// before nvnDeviceInitialize; without it the table above answers.
pub fn set_sdk_image(img: &[u8]) {
    let items = nvn2_items(img);
    match &items {
        Some(v) => crate::nvnlog!("sdk: NVN2's nvnDeviceGetInteger switch found ({} items, {} constant)", v.len(), v.iter().filter(|x| x.is_some()).count()),
        None if SWITCH2.load(Ordering::Relaxed) => crate::log::red("sdk: NVN2's nvnDeviceGetInteger switch not found; the device answers the newest versions known"),
        None => {}
    }
    let _ = NVN2_ITEMS.set(items.unwrap_or_default());
}

/// The driver's switch over the items loads w0 with the major (mov w0, #231) and jumps through a byte table: cmp wN,
/// #last / adr (the cases) / adrp + add (the table) / ldrb / add, lsl #2 / br. A constant case is `mov w0, #v; b`.
fn nvn2_items(img: &[u8]) -> Option<Vec<Option<i32>>> {
    let n = img.len() / 4;
    let w = |i: usize| u32::from_le_bytes(img[i * 4..i * 4 + 4].try_into().unwrap());
    let imm21 = |x: u32| -> i64 {
        let v = ((x >> 3) & 0x1FFFFC) as i64 | ((x >> 29) & 3) as i64;
        if v & 0x100000 != 0 {
            v - 0x200000
        } else {
            v
        }
    };
    for p in 8..n.saturating_sub(9) {
        if w(p) != 0x52801CE0 {
            continue;
        }
        let (mut tb, mut adr, mut lim, mut ldrb, mut br) =
            (None::<usize>, None::<usize>, None::<usize>, false, false);
        for q in p - 8..=p + 8 {
            let x = w(q);
            if x & 0xFFC0001F == 0x7100001F {
                lim = Some(((x >> 10) & 0xFFF) as usize);
            } else if x & 0x9F000000 == 0x90000000 && w(q + 1) & 0xFFC00000 == 0x91000000 {
                let page = (q * 4) as i64 & !0xFFF;
                tb = usize::try_from(page + imm21(x) * 4096 + ((w(q + 1) >> 10) & 0xFFF) as i64)
                    .ok();
            } else if x & 0x9F000000 == 0x10000000 {
                adr = usize::try_from((q * 4) as i64 + imm21(x)).ok();
            } else if x & 0xFFE0FC00 == 0x38606800 {
                ldrb = true;
            } else if x & 0xFFFFFC1F == 0xD61F0000 {
                br = true;
            }
        }
        let (Some(tb), Some(adr), Some(lim)) = (tb, adr, lim) else {
            continue;
        };
        if lim <= 1 || !ldrb || !br || tb + lim >= img.len() {
            continue;
        }
        let items = (0..=lim)
            .map(|item| {
                let c = adr + 4 * img[tb + item] as usize;
                if c + 8 > img.len() {
                    return None;
                }
                let (c0, c1) = (
                    u32::from_le_bytes(img[c..c + 4].try_into().unwrap()),
                    u32::from_le_bytes(img[c + 4..c + 8].try_into().unwrap()),
                );
                (c0 & 0xFFE0001F == 0x52800000 && c1 & 0xFC000000 == 0x14000000)
                    .then_some(((c0 >> 5) & 0xFFFF) as i32)
            })
            .collect();
        return Some(items);
    }
    None
}

fn nvn2_item(w: usize) -> Option<i32> {
    NVN2_ITEMS.get().and_then(|v| v.get(w).copied().flatten())
}

/// NVN counter timestamps: 614.4 MHz ticks on the Switch, nanoseconds on the Switch 2.
pub fn timestamp_from_ns(ns: u64) -> u64 {
    if SWITCH2.load(Ordering::Relaxed) {
        ns
    } else {
        (ns as u128 * 384 / 625) as u64
    }
}
pub fn timestamp_to_ns(t: u64) -> u64 {
    if SWITCH2.load(Ordering::Relaxed) {
        t
    } else {
        (t as u128 * 625 / 384) as u64
    }
}

/// NVN2's bindless uniform buffer handle in the hardware's layout (Ampere): bits 0..44 the address >> 4, 45..63 the
/// size in 16-byte units, capped at 2^19 - 1 (nxnative `ucb_handle`). The driver writes it for
/// nvnDeviceGetUniformBufferHandle and into compute's uniform table in c0.
pub fn ucb_handle(addr: u64, size: u64) -> u64 {
    let s = size.div_ceil(16).min((1 << 19) - 1);
    (addr >> 4 & ((1 << 45) - 1)) | s << 45
}

/// The address a word holding either a plain address or a `ucb_handle` names: a handle of any size has a bit at 45
/// or above, which no guest address has (nxnative `ucb_handle_addr`). Whether it was a handle is returned too.
pub fn ucb_handle_addr(v: u64) -> (u64, bool) {
    if v >> 45 != 0 {
        ((v & ((1 << 45) - 1)) << 4, true)
    } else {
        (v, false)
    }
}

/// nvnDeviceGetCurrentTimestampInNanoseconds: the guest clock, as the counter reports and the tick counter use (and
/// 256 times slower with `game_dynamic_resolution=0`, as the reports are then).
pub fn current_timestamp_ns() -> u64 {
    let ns = crate::vk::counters::guest_ns();
    if crate::vk::counters::drs() {
        ns
    } else {
        ns / 256
    }
}

pub fn now_ns() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

/// nvnDeviceGetInteger's answer for item `w` (`s2`: the Switch 2's driver).
fn device_integer(w: usize, s2: bool) -> i32 {
    if w <= 1 {
        let a = api();
        if w == 0 { a.0 } else { a.1 }
    } else if s2 && w == 6 {
        // vertex buffer bindings: the host's tables hold 16 (BindVertexBuffers refuses more), whatever the sdk says
        16
    } else if s2 && w < DEVINFO2.len() && (DEVINFO2[w] != RT || nvn2_item(w).is_some() || w >= DEVINFO.len()) {
        // the Switch 2's driver: its own constant where its switch has one (GPU code versions move with the SDK)
        if DEVINFO2[w] == RT && nvn2_item(w).is_none() {
            crate::log::red_once(&format!("nvnDeviceGetInteger item {w:#x}: NVN2 reads it from the GPU at run time, not known here; returning 0"));
            0
        } else {
            nvn2_item(w).unwrap_or(DEVINFO2[w])
        }
    } else if w == 0x67 {
        // SDK 19 (FINAL FANTASY TACTICS): the alignment of its copies of a shader's control section and code
        256
    } else if w == 0x2E || w == 0x30 {
        // the GPU code version the shader translation reads (1.16)
        if w == 0x2E { 1 } else { 16 }
    } else if w < DEVINFO.len() {
        DEVINFO[w]
    } else {
        crate::log::red_once(&format!("nvnDeviceGetInteger item {w:#x}: unknown item; returning 0"));
        0
    }
}

fn api() -> (i32, i32) {
    if SWITCH2.load(Ordering::Relaxed) {
        return (231, nvn2_item(1).unwrap_or(DEVINFO2[1]));
    }
    let m = SDK_MAJOR.load(Ordering::Relaxed);
    if m != 0 && m <= 10 {
        (53, 313)
    } else {
        (55, 15)
    }
}

nvn_fns! {
    fn nvnDeviceBuilderSetDefaults(b: *mut NVNdeviceBuilder) { (*(b as *mut dev_builder_t)).flags = 0; }
    fn nvnDeviceBuilderSetFlags(b: *mut NVNdeviceBuilder, f: i32) { (*(b as *mut dev_builder_t)).flags = f as u32; }
    fn nvnDeviceBuilderGetFlags(b: *const NVNdeviceBuilder) -> i32 { (*(b as *const dev_builder_t)).flags as i32 }
    fn nvnDeviceInitialize(d: *mut NVNdevice, b: *const NVNdeviceBuilder) -> NVNboolean {
        let o = d as *mut dev_t;
        core::ptr::write_bytes(o as *mut u8, 0, core::mem::size_of::<dev_t>());
        (*o).magic = MAGIC_DEV;
        (*o).flags = (*(b as *const dev_builder_t)).flags;
        (*o).origin = origin::LOWER_LEFT;
        ORIGIN_UPPER_LEFT.store(false, Ordering::Relaxed);
        DEPTH_ZERO.store(false, Ordering::Relaxed);
        crate::tex::deswizzle::GOB_SWITCH2.store(SWITCH2.load(Ordering::Relaxed), Ordering::Relaxed);
        match crate::vk::Gpu::get() {
            Ok(_) => 1,
            Err(e) => { crate::log::red(&format!("nvnDeviceInitialize: no Vulkan device: {e}")); 0 }
        }
    }
    fn nvnDeviceFinalize(d: *mut NVNdevice) { (*(d as *mut dev_t)).magic = 0; }
    fn nvnDeviceGetProcAddress(d: *const NVNdevice, name: *const u8) -> u64 { let _ = d; super::resolve(&super::cstr(name)) }
    fn nvnDeviceGetInteger(d: *const NVNdevice, what: i32, out: *mut i32) {
        let _ = d;
        *out = device_integer(what as u32 as usize, SWITCH2.load(Ordering::Relaxed));
    }
    /// The GPU's current time on the clock its counter timestamps report (`counters::report_time`): the guest clock.
    /// It had graphics' own epoch, and a frame-rate controller comparing the two read a wrong GPU time (nxnative
    /// hle_nvn.c: Donkey Kong Bananza held at 20 FPS by such a comparison).
    fn nvnDeviceGetCurrentTimestampInNanoseconds(d: *const NVNdevice) -> u64 { let _ = d; current_timestamp_ns() }
    fn nvnDeviceGetTimestampInNanoseconds(d: *const NVNdevice, c: *const NVNcounterData) -> u64 { let _ = d; timestamp_to_ns((*c).timestamp) }
    fn nvnDeviceGetTextureHandle(d: *const NVNdevice, tex: i32, samp: i32) -> u64 { let _ = d; (tex as u32 as u64 & 0xfffff) | (samp as u32 as u64) << 20 }
    fn nvnDeviceGetTexelFetchHandle(d: *const NVNdevice, tex: i32) -> u64 { let _ = d; tex as u32 as u64 & 0xfffff }
    fn nvnDeviceGetImageHandle(d: *const NVNdevice, id: i32) -> u64 { let _ = d; id as u32 as u64 }
    fn nvnDeviceGetSeparateTextureHandle(d: *const NVNdevice, id: i32) -> NVNseparateTextureHandle { let _ = d; NVNseparateTextureHandle { value: id as u32 as u64 } }
    fn nvnDeviceGetSeparateSamplerHandle(d: *const NVNdevice, id: i32) -> NVNseparateSamplerHandle { let _ = d; NVNseparateSamplerHandle { value: (id as u32 as u64) << 20 } }
    fn nvnDeviceInstallDebugCallback(d: *mut NVNdevice, cb: u64, data: u64, enable: u32) {
        let o = d as *mut dev_t;
        (*o).dbg_cb = if super::b(enable) { cb } else { 0 };
        (*o).dbg_data = data;
    }
    fn nvnDeviceSetWindowOriginMode(d: *mut NVNdevice, m: u32) {
        (*(d as *mut dev_t)).origin = m;
        ORIGIN_UPPER_LEFT.store(m == origin::UPPER_LEFT, Ordering::Relaxed);
    }
    fn nvnDeviceSetDepthMode(d: *mut NVNdevice, m: u32) {
        (*(d as *mut dev_t)).depth_mode = m;
        DEPTH_ZERO.store(m != 0, Ordering::Relaxed);
    }
    fn nvnDeviceGetWindowOriginMode(d: *const NVNdevice) -> u32 { (*(d as *const dev_t)).origin }
    fn nvnDeviceGetDepthMode(d: *const NVNdevice) -> u32 { (*(d as *const dev_t)).depth_mode }
    fn nvnDeviceFinalizeCommandHandle(d: *mut NVNdevice, h: u64) { let _ = (d, h); }
    fn nvnDeviceGenerateDebugDomainId(d: *const NVNdevice, name: *const u8) -> i32 {
        let _ = (d, name);
        static NEXT: AtomicI32 = AtomicI32::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed) + 1
    }
}

#[cfg(test)]
mod tests {
    /// The Switch 2's device: 16 vertex buffer bindings (the host's tables), 0x62 the X1 table's 32 (nxnative), and
    /// a run-time item past the X1 table still 0.
    #[test]
    fn switch2_items_the_host_answers() {
        assert_eq!(super::device_integer(6, true), 16);
        assert_eq!(super::device_integer(0x62, true), 32);
        assert_eq!(super::device_integer(0x62, false), 32);
        assert_eq!(super::device_integer(0x7D, true), 0);
        assert_eq!(super::device_integer(0x0E, true), 32, "a constant of NVN2's own still answers");
        assert_eq!(super::device_integer(6, false), 16);
    }

    /// The hardware's bindless uniform buffer handle and its decode (nxnative ucb_handle / ucb_handle_addr): PRAGMATA
    /// pushes 0x5a0083055d124: 0x83055d1240 with size field 0x2d (720 bytes); the size field is capped; a plain
    /// address passes through.
    #[test]
    fn uniform_buffer_handles_round_trip() {
        assert_eq!(super::ucb_handle(0x83055d1240, 720), 0x5a0083055d124);
        assert_eq!(super::ucb_handle_addr(0x5a0083055d124), (0x83055d1240, true));
        assert_eq!(super::ucb_handle_addr(0x83055d1240), (0x83055d1240, false));
        assert_eq!(super::ucb_handle(0x1000, 1 << 40) >> 45, (1 << 19) - 1, "size capped at 2^19 - 1");
        assert_eq!(super::ucb_handle_addr(super::ucb_handle(0x1000, 1 << 40)).0, 0x1000);
    }
}

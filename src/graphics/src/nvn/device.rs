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
    55, 15, 14, 65536, 256, 8, 16, 4, 16, 32, 16, 4, 4, 4, 16, 32, 32, 256, 256, 4, 8, 512, 256, 2, 256, 4096, 131072, 16, 1536, 1024, 64,
    1536, 65535, 65535, 65535, 8, 1048576, 4096, 16, 4096, 1, 1, 1, 262144, 15, 2048, 1, 1, 14, 5, 1, 8, 8, 4, 32, 128, 32, 128, 65536,
    1, 4, 16384, 134217728, 2048, 16384, 16384, 1, 1, 1, 1, 5, 1, 1, 1, 1, 1, 1, 16, 1024, 32, 4096, 65536, 65536, 256, 16384, 262144,
    4096, 1, 4, 2, 0, 4096, 16384, 4096, 128, 32, 65535, 1, 32, 256, 16, 65535, 65535, 256, 16383,
];

/// NVN counter timestamps: 614.4 MHz ticks on the Switch, nanoseconds on the Switch 2.
pub fn timestamp_from_ns(ns: u64) -> u64 {
    if SWITCH2.load(Ordering::Relaxed) { ns } else { (ns as u128 * 384 / 625) as u64 }
}
pub fn timestamp_to_ns(t: u64) -> u64 {
    if SWITCH2.load(Ordering::Relaxed) { t } else { (t as u128 * 625 / 384) as u64 }
}

pub fn now_ns() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

fn api() -> (i32, i32) {
    if SWITCH2.load(Ordering::Relaxed) {
        return (231, 39);
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
        let w = what as u32 as usize;
        let v = if w <= 1 {
            let a = api();
            if w == 0 { a.0 } else { a.1 }
        } else if w == 0x2E || w == 0x30 {
            // the GPU code version the shader translation reads (1.16)
            if w == 0x2E { 1 } else { 16 }
        } else if w < DEVINFO.len() {
            DEVINFO[w]
        } else {
            crate::log::red_once(&format!("nvnDeviceGetInteger item {w:#x}: unknown item; returning 0"));
            0
        };
        *out = v;
    }
    fn nvnDeviceGetCurrentTimestampInNanoseconds(d: *const NVNdevice) -> u64 { let _ = d; now_ns() }
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

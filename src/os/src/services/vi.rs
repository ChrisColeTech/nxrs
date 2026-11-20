//! nn::vi: the display, the layer and the native window handed to NVN's window builder are the host's objects
//! (small guest allocations), the layer crop is kept for the present, and the frame number is the count of
//! presents.

use crate::guest::{w32, w64, Args};
use crate::hle::{Registry, RES_FAIL, RES_OK};
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

fn obj(slot: usize, magic: u32) -> u64 {
    static OBJ: [OnceLock<u64>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    *OBJ[slot].get_or_init(|| {
        let a = nxrt::kernel::get().space.kernel_alloc(0x40, 16);
        w32(a, magic);
        a
    })
}
pub fn display() -> u64 {
    obj(0, 0x5053_4944)
}
pub fn layer() -> u64 {
    obj(1, 0x5259_414c)
}
pub fn native_window() -> u64 {
    obj(2, 0x4e49_574e)
}

/// The layer's size from a sized CreateLayer (w | h << 16), and its crop (x | y << 16 | w << 32 | h << 48).
pub static LAYER_WH: AtomicU64 = AtomicU64::new(0);
pub static CROP: AtomicU64 = AtomicU64::new(0);

macro_rules! vifn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

vifn!(nothing, |a| { 0 });
vifn!(ok, |a| { RES_OK });
vifn!(open_default, |a| {
    w64(a.x(0), display());
    RES_OK
});
vifn!(open_named, |a| {
    if crate::guest::cstr(a.x(1), 64) != "Default" {
        return RES_FAIL;
    }
    w64(a.x(0), display());
    RES_OK
});
vifn!(create_layer, |a| {
    w64(a.x(0), layer());
    RES_OK
});
vifn!(create_layer_wh, |a| {
    let (w, h) = (a.i(2), a.i(3));
    crate::log!("vi::CreateLayer {w}x{h}");
    if w > 0 && h > 0 && w < 0x10000 && h < 0x10000 {
        LAYER_WH.store(w as u64 | (h as u64) << 16, Ordering::Relaxed);
    }
    w64(a.x(0), layer());
    RES_OK
});
vifn!(native, |a| {
    w64(a.x(0), native_window());
    RES_OK
});
vifn!(set_crop, |a| {
    let (x, y, w, h) = (a.i(1), a.i(2), a.i(3), a.i(4));
    let v = if x < 0 || y < 0 || w <= 0 || h <= 0 || x > 0xffff || y > 0xffff || w > 0xffff || h > 0xffff {
        0
    } else {
        x as u64 | (y as u64) << 16 | (w as u64) << 32 | (h as u64) << 48
    };
    if CROP.swap(v, Ordering::Relaxed) != v {
        crate::log!("vi::SetLayerCrop {x},{y} {w}x{h}");
    }
    RES_OK
});
vifn!(scaling, |a| {
    crate::log_once!("vi scaling", "vi::SetLayerScalingMode {} (the host present scales itself)", a.i(1));
    RES_OK
});
vifn!(frame_number, |a| {
    w64(a.x(0), crate::nvn::presents());
    RES_OK
});
vifn!(list_displays, |a| {
    let (out, n) = (a.x(0), a.i(1));
    if n <= 0 || out == 0 {
        return 0;
    }
    crate::guest::zero(out, 0x60);
    crate::guest::put_cstr(out, 0x40, "Default");
    crate::guest::w8(out + 0x40, 1);
    w64(out + 0x48, 1);
    w64(out + 0x50, 1920);
    w64(out + 0x58, 1080);
    1
});
vifn!(panel_id, |a| {
    crate::guest::zero(a.x(0), 0x18);
    RES_OK
});
// SDK 22's CreateLayer(Layer**, Display*, const LayerCreationSettings*): settings start {w, h, ...}; the SDK's own
// version goes to an am self controller the host never made.
vifn!(create_layer_settings, |a| {
    let s = a.x(2);
    let (w, h) = if s != 0 { (crate::guest::r32(s) as i32, crate::guest::r32(s + 4) as i32) } else { (0, 0) };
    crate::log!("vi::CreateLayer {w}x{h} (creation settings)");
    if w > 0 && h > 0 && w < 0x10000 && h < 0x10000 {
        LAYER_WH.store(w as u64 | (h as u64) << 16, Ordering::Relaxed);
    }
    w64(a.x(0), layer());
    RES_OK
});

// ---- Switch 2 SDKs: HDR, VRR, extended swap intervals. The SDK's versions forward the layer to vi2 through a table
// its graphics system fills, which is null here (Mario Kart World: a call through null from
// SetLayerNextHdrContentGamutMetadata 73 s in, main+0x959a44 -> sdk+0x5d6f90). The host presents SDR at a fixed
// refresh rate, so the metadata has nothing to apply to.
vifn!(hdr_status, |a| {
    // (bool *on, HdrOutputFormat *fmt, u16 *peak, const Display*): SDR
    if a.x(0) != 0 {
        crate::guest::w8(a.x(0), 0);
    }
    if a.x(1) != 0 {
        w32(a.x(1), 0);
    }
    if a.x(2) != 0 {
        crate::guest::w16(a.x(2), 0);
    }
    RES_OK
});
vifn!(paper_white, |a| {
    // BT.2408's reference white, read only when HDR is on
    if a.x(0) != 0 {
        crate::guest::w16(a.x(0), 203);
    }
    RES_OK
});
vifn!(vrr_supported, |a| {
    if a.x(0) != 0 {
        crate::guest::w8(a.x(0), 0);
    }
    RES_OK
});
fn screen_pixels() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}
// BufferMaxResolution: 0 1280x720, 1 1920x1080, 2 2560x1440, 3 3840x2160. Handheld: the Switch 2's own 1080p screen;
// docked: the largest of the four the host's display holds (1080p when unknown).
vifn!(buffer_max_res, |a| {
    const K: [(i32, i32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
    let mut r = 1usize;
    let (mut w, mut h) = (0, 0);
    if crate::services::oe::opmode() == 1 {
        (w, h) = screen_pixels();
        if w < h {
            std::mem::swap(&mut w, &mut h);
        }
        if w > 0 && h > 0 {
            r = 0;
            for (i, k) in K.iter().enumerate() {
                if w >= k.0 && h >= k.1 {
                    r = i;
                }
            }
        }
    }
    // NXOS_DISPLAY_MAX=0..3 caps it (debug: a game's 2160 layer against its 1440 one)
    if let Some(m) = std::env::var("NXOS_DISPLAY_MAX").ok().and_then(|v| v.parse::<usize>().ok()) {
        r = r.min(m.min(3));
    }
    crate::log_once!("vi bufmax", "vi::QueryDisplayBufferMaxResolution: {}x{} ({}, display {w}x{h})", K[r].0, K[r].1, if crate::services::oe::opmode() == 1 { "docked" } else { "handheld" });
    if a.x(0) != 0 {
        w32(a.x(0), r as u32);
    }
    RES_OK
});
vifn!(ext_swap, |a| {
    crate::log_once!("vi extswap", "vi::SetLayerExtendedSwapInterval {} {}: no VRR on the host, the NVN swap interval stays", a.i(1), a.i(2));
    RES_OK
});
vifn!(hdr_meta, |a| {
    crate::log_once!("vi hdr meta", "vi: layer HDR metadata set / cleared: the host presents SDR, nothing to apply");
    RES_OK
});
// (size_t *size, size_t *align, int w, int h): an indirect layer's image, RGBA8 linear, 4 KB aligned
vifn!(indirect_req, |a| {
    let (w, h) = (a.i(2).max(0) as u64, a.i(3).max(0) as u64);
    w64(a.x(0), (w * h * 4 + 0xfff) & !0xfff);
    w64(a.x(1), 0x1000);
    RES_OK
});

pub fn register(r: &mut Registry) {
    const W: &str = "vi: the host's display, layer and native window";
    for (n, f) in [
        ("_ZN2nn2vi10InitializeEv", nothing as unsafe fn(*mut Cpu)),
        ("_ZN2nn2vi8FinalizeEv", nothing),
        ("_ZN2nn2vi18OpenDefaultDisplayEPPNS0_7DisplayE", open_default),
        ("_ZN2nn2vi11OpenDisplayEPPNS0_7DisplayEPKc", open_named),
        ("_ZN2nn2vi11CreateLayerEPPNS0_5LayerEPNS0_7DisplayE", create_layer),
        ("_ZN2nn2vi11CreateLayerEPPNS0_5LayerEPNS0_7DisplayEii", create_layer_wh),
        ("_ZN2nn2vi15GetNativeWindowEPPvPNS0_5LayerE", native),
        ("_ZN2nn2vi12SetLayerCropEPNS0_5LayerEiiii", set_crop),
        ("_ZN2nn2vi19SetLayerScalingModeEPNS0_5LayerENS0_11ScalingModeE", scaling),
        ("_ZN2nn2vi20GetLatestFrameNumberEPmPKNS0_5LayerE", frame_number),
        ("_ZN2nn2vi12ListDisplaysEPNS0_11DisplayInfoEi", list_displays),
        ("_ZN2nn2vi32GetDefaultDisplayPanelIdentifierEPNS0_15PanelIdentifierE", panel_id),
        ("_ZN2nn2vi12DestroyLayerEPNS0_5LayerE", nothing),
        ("_ZN2nn2vi12CloseDisplayEPNS0_7DisplayE", nothing),
        ("_ZN2nn2vi23SetDisplayVrrCapabilityEPNS0_7DisplayEb", ok),
        ("_ZN2nn2vi27InitializeWithVrrLfcEnabledEv", nothing),
        ("_ZN2nn2vi11CreateLayerEPPNS0_5LayerEPNS0_7DisplayEPKNS0_21LayerCreationSettingsE", create_layer_settings),
        ("_ZN2nn2vi21QueryDisplayHdrStatusEPbPNS0_15HdrOutputFormatEPtPKNS0_7DisplayE", hdr_status),
        ("_ZN2nn2vi25QueryDisplayHdrPaperWhiteEPtPKNS0_7DisplayE", paper_white),
        ("_ZN2nn2vi24QueryDisplayVrrSupportedEPbPKNS0_7DisplayE", vrr_supported),
        ("_ZN2nn2vi31QueryDisplayBufferMaxResolutionEPNS0_19BufferMaxResolutionEPKNS0_7DisplayE", buffer_max_res),
        ("_ZN2nn2vi28SetLayerExtendedSwapIntervalEPNS0_5LayerEii", ext_swap),
        ("_ZN2nn2vi30ClearLayerExtendedSwapIntervalEPNS0_5LayerE", ok),
        ("_ZN2nn2vi35SetLayerNextHdrContentGamutMetadataEPNS0_5LayerENS0_15HdrContentGamutE", hdr_meta),
        ("_ZN2nn2vi39SetLayerNextHdrToSdrToneMappingMetadataEPNS0_5LayerENS0_14HdrToneMappingE", hdr_meta),
        ("_ZN2nn2vi52SetLayerNextHdrExtendedContentLuminanceRangeMetadataEPNS0_5LayerEtt", hdr_meta),
        ("_ZN2nn2vi52SetLayerNextHdrStandardContentLuminanceRangeMetadataEPNS0_5LayerEtt", hdr_meta),
        ("_ZN2nn2vi39SetLayerNextHdrAdjustableBezierMetadataEPNS0_5LayerEfff", hdr_meta),
        ("_ZN2nn2vi41ClearLayerNextHdrAdjustableBezierMetadataEPNS0_5LayerE", hdr_meta),
        ("_ZN2nn2vi37ClearLayerNextHdrContentGamutMetadataEPNS0_5LayerE", hdr_meta),
        ("_ZN2nn2vi41ClearLayerNextHdrToSdrToneMappingMetadataEPNS0_5LayerE", hdr_meta),
        ("_ZN2nn2vi33GetIndirectImageMemoryRequirementEPmS1_ii", indirect_req),
    ] {
        r.force(n, W, f);
    }
}

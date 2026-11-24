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
    let v = if x < 0
        || y < 0
        || w <= 0
        || h <= 0
        || x > 0xffff
        || y > 0xffff
        || w > 0xffff
        || h > 0xffff
    {
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
    crate::log_once!(
        "vi scaling",
        "vi::SetLayerScalingMode {} (the host present scales itself)",
        a.i(1)
    );
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
    let (w, h) = if s != 0 {
        (crate::guest::r32(s) as i32, crate::guest::r32(s + 4) as i32)
    } else {
        (0, 0)
    };
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
#[cfg(windows)]
fn screen_pixels() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// The primary monitor's size in pixels, as the front end's window system reports it (Linux has no system-wide
/// query without a window connection): width << 32 | height, 0 unknown.
pub static HOST_SCREEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(not(windows))]
fn screen_pixels() -> (i32, i32) {
    let v = HOST_SCREEN.load(std::sync::atomic::Ordering::Acquire);
    ((v >> 32) as i32, v as u32 as i32)
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
    if let Some(m) = std::env::var("NXOS_DISPLAY_MAX")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        r = r.min(m.min(3));
    }
    crate::log_once!(
        "vi bufmax",
        "vi::QueryDisplayBufferMaxResolution: {}x{} ({}, display {w}x{h})",
        K[r].0,
        K[r].1,
        if crate::services::oe::opmode() == 1 {
            "docked"
        } else {
            "handheld"
        }
    );
    if a.x(0) != 0 {
        w32(a.x(0), r as u32);
    }
    RES_OK
});
vifn!(ext_swap, |a| {
    crate::log_once!(
        "vi extswap",
        "vi::SetLayerExtendedSwapInterval {} {}: no VRR on the host, the NVN swap interval stays",
        a.i(1),
        a.i(2)
    );
    RES_OK
});
vifn!(hdr_meta, |a| {
    crate::log_once!(
        "vi hdr meta",
        "vi: layer HDR metadata set / cleared: the host presents SDR, nothing to apply"
    );
    RES_OK
});
// (size_t *size, size_t *align, int w, int h): an indirect layer's image, RGBA8 linear, 4 KB aligned
vifn!(indirect_req, |a| {
    let (w, h) = (a.i(2).max(0) as u64, a.i(3).max(0) as u64);
    w64(a.x(0), (w * h * 4 + 0xfff) & !0xfff);
    w64(a.x(1), 0x1000);
    RES_OK
});

// ---- the layer's frame history (nxnative hle_vi.c): the last 8 presents, newest first

const VFH: usize = 8;

#[derive(Clone, Copy, Default)]
struct FrameRec {
    frame: u64,
    queued: u64,
    shown: u64,
}

static HISTORY: std::sync::Mutex<[FrameRec; VFH]> = std::sync::Mutex::new(
    [FrameRec {
        frame: 0,
        queued: 0,
        shown: 0,
    }; VFH],
);

/// At each present the game queues (`nvn::note_present`): frame = the frame number with it, at the guest counter now.
pub fn frame_queued(frame: u64) {
    HISTORY.lock().unwrap()[frame as usize % VFH] = FrameRec {
        frame,
        queued: crate::svc::ticks(),
        shown: 0,
    };
}

/// FrameInfo[] (40 bytes each: status 2 shown / 1 queued, frame number, the queue tick, the present tick twice), the
/// frames up to `latest`, newest first; a frame is shown once the present queue has handed it to the display
/// (`handed`), at the tick it was first seen so. Returns the entries written.
fn frame_history(out: u64, n: usize, latest: u64, handed: u64, now: u64) -> usize {
    let mut h = HISTORY.lock().unwrap();
    let mut k = 0;
    let mut f = latest;
    while f > 0 && latest - f < VFH as u64 && k < n {
        let e = &mut h[f as usize % VFH];
        if e.frame == f {
            if f <= handed && e.shown == 0 {
                e.shown = now.max(1);
            }
            let o = out + 40 * k as u64;
            w64(o, if e.shown != 0 { 2 } else { 1 });
            w64(o + 8, f);
            w64(o + 16, e.queued);
            w64(o + 24, e.shown);
            w64(o + 32, e.shown);
            k += 1;
        }
        f -= 1;
    }
    k
}

// ListFrameHistory(FrameInfo *out, int n, const Layer *) -> int: a null buffer asks for the history's size (8).
// Unreal Engine 5's flip tracking reads it each vsync and completes its frame-end fences by the first shown entry;
// the SDK's own version reads the compositor's records the host never made.
vifn!(list_frame_history, |a| {
    let (out, n) = (a.x(0), a.i(1));
    if out == 0 || n <= 0 {
        return VFH as u64;
    }
    let k = frame_history(
        out,
        n as usize,
        crate::nvn::presents(),
        nxgfx::vk::present::presented(),
        crate::svc::ticks(),
    );
    crate::log_once!(
        "vi history",
        "vi::ListFrameHistory: {k} of the last {VFH} presents returned"
    );
    k as u64
});

// GetIndirectImage(u64 *w, u64 *h, void *buffer, size_t size, u64 consumer, int width, int height[, 4 floats: the
// crop]): the picture of an indirect layer (the inline keyboard's applet draws into it; the game draws it over its
// screen). The host's keyboard is a window of its own, so the picture is fully transparent (Turbo vi 2450 / 2451:
// the buffer zeroed, sizes 0); left as it was, whatever that memory held would be drawn over the game.
vifn!(indirect_image, |a| {
    let (buf, size) = (a.x(2), a.x(3));
    if buf != 0 && size != 0 && size <= 64 << 20 {
        crate::guest::zero(buf, size);
    }
    for i in 0..2 {
        if a.x(i) != 0 {
            w64(a.x(i), 0);
        }
    }
    crate::log_once!(
        "vi indirect",
        "vi::GetIndirectImage {}x{}: transparent (the host's keyboard is a window of its own)",
        a.i(5),
        a.i(6)
    );
    RES_OK
});

pub fn register(r: &mut Registry) {
    const W: &str = "vi: the host's display, layer and native window";
    for (n, f) in [
        ("_ZN2nn2vi10InitializeEv", nothing as unsafe fn(*mut Cpu)),
        ("_ZN2nn2vi8FinalizeEv", nothing),
        (
            "_ZN2nn2vi18OpenDefaultDisplayEPPNS0_7DisplayE",
            open_default,
        ),
        ("_ZN2nn2vi11OpenDisplayEPPNS0_7DisplayEPKc", open_named),
        (
            "_ZN2nn2vi11CreateLayerEPPNS0_5LayerEPNS0_7DisplayE",
            create_layer,
        ),
        (
            "_ZN2nn2vi11CreateLayerEPPNS0_5LayerEPNS0_7DisplayEii",
            create_layer_wh,
        ),
        ("_ZN2nn2vi15GetNativeWindowEPPvPNS0_5LayerE", native),
        ("_ZN2nn2vi12SetLayerCropEPNS0_5LayerEiiii", set_crop),
        (
            "_ZN2nn2vi19SetLayerScalingModeEPNS0_5LayerENS0_11ScalingModeE",
            scaling,
        ),
        (
            "_ZN2nn2vi20GetLatestFrameNumberEPmPKNS0_5LayerE",
            frame_number,
        ),
        (
            "_ZN2nn2vi12ListDisplaysEPNS0_11DisplayInfoEi",
            list_displays,
        ),
        (
            "_ZN2nn2vi32GetDefaultDisplayPanelIdentifierEPNS0_15PanelIdentifierE",
            panel_id,
        ),
        ("_ZN2nn2vi12DestroyLayerEPNS0_5LayerE", nothing),
        ("_ZN2nn2vi12CloseDisplayEPNS0_7DisplayE", nothing),
        ("_ZN2nn2vi23SetDisplayVrrCapabilityEPNS0_7DisplayEb", ok),
        ("_ZN2nn2vi27InitializeWithVrrLfcEnabledEv", nothing),
        (
            "_ZN2nn2vi11CreateLayerEPPNS0_5LayerEPNS0_7DisplayEPKNS0_21LayerCreationSettingsE",
            create_layer_settings,
        ),
        (
            "_ZN2nn2vi21QueryDisplayHdrStatusEPbPNS0_15HdrOutputFormatEPtPKNS0_7DisplayE",
            hdr_status,
        ),
        (
            "_ZN2nn2vi25QueryDisplayHdrPaperWhiteEPtPKNS0_7DisplayE",
            paper_white,
        ),
        (
            "_ZN2nn2vi24QueryDisplayVrrSupportedEPbPKNS0_7DisplayE",
            vrr_supported,
        ),
        (
            "_ZN2nn2vi31QueryDisplayBufferMaxResolutionEPNS0_19BufferMaxResolutionEPKNS0_7DisplayE",
            buffer_max_res,
        ),
        (
            "_ZN2nn2vi28SetLayerExtendedSwapIntervalEPNS0_5LayerEii",
            ext_swap,
        ),
        ("_ZN2nn2vi30ClearLayerExtendedSwapIntervalEPNS0_5LayerE", ok),
        (
            "_ZN2nn2vi35SetLayerNextHdrContentGamutMetadataEPNS0_5LayerENS0_15HdrContentGamutE",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi39SetLayerNextHdrToSdrToneMappingMetadataEPNS0_5LayerENS0_14HdrToneMappingE",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi52SetLayerNextHdrExtendedContentLuminanceRangeMetadataEPNS0_5LayerEtt",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi52SetLayerNextHdrStandardContentLuminanceRangeMetadataEPNS0_5LayerEtt",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi39SetLayerNextHdrAdjustableBezierMetadataEPNS0_5LayerEfff",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi41ClearLayerNextHdrAdjustableBezierMetadataEPNS0_5LayerE",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi37ClearLayerNextHdrContentGamutMetadataEPNS0_5LayerE",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi41ClearLayerNextHdrToSdrToneMappingMetadataEPNS0_5LayerE",
            hdr_meta,
        ),
        (
            "_ZN2nn2vi33GetIndirectImageMemoryRequirementEPmS1_ii",
            indirect_req,
        ),
        (
            "_ZN2nn2vi16ListFrameHistoryEPNS0_9FrameInfoEiPKNS0_5LayerE",
            list_frame_history,
        ),
        ("_ZN2nn2vi16GetIndirectImageEPmS1_Pvmmii", indirect_image),
        (
            "_ZN2nn2vi16GetIndirectImageEPmS1_Pvmmiiffff",
            indirect_image,
        ),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    /// ListFrameHistory as nxnative answers it: the size (8) for a null buffer; the frames up to the latest, newest
    /// first, status 2 with a present tick once the present queue handed them on, 1 before; frames older than 8 are
    /// gone. GetIndirectImage leaves a transparent picture (Turbo 2450).
    #[test]
    fn frame_history_and_indirect_image() {
        for f in 1..=10 {
            super::frame_queued(f);
        }
        let mut out = vec![0u64; 5 * 10];
        let a = out.as_mut_ptr() as u64;
        // frames 1..10 queued, 9 handed to the display
        assert_eq!(super::frame_history(a, 10, 10, 9, 777), 8, "8 at most");
        assert_eq!(&out[0..2], &[1, 10], "the newest, not shown yet");
        assert_eq!(
            (out[4], out[5], out[6], out[8], out[9]),
            (0, 2, 9, 777, 777),
            "shown at the tick first seen so"
        );
        assert_eq!(out[7 * 5 + 1], 3);
        assert_eq!(super::frame_history(a, 2, 10, 10, 900), 2);
        assert_eq!(
            (out[0], out[3], out[5], out[8]),
            (2, 900, 2, 777),
            "a shown frame keeps its first tick"
        );
        let mut c = nxrt::cpu::Cpu::new_boxed();
        c.x[0] = 0;
        c.x[1] = 0;
        unsafe { super::list_frame_history(&mut *c) };
        assert_eq!(c.x[0], 8);
        let mut pic = vec![0xFFu8; 4096];
        let (mut w, mut h) = (5u64, 5u64);
        c.x[..7].copy_from_slice(&[
            &mut w as *mut u64 as u64,
            &mut h as *mut u64 as u64,
            pic.as_mut_ptr() as u64,
            4096,
            1,
            32,
            32,
        ]);
        unsafe { super::indirect_image(&mut *c) };
        assert_eq!((c.x[0], w, h), (0, 0, 0));
        assert!(pic.iter().all(|&b| b == 0));
    }
}

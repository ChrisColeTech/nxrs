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
    ] {
        r.force(n, W, f);
    }
}

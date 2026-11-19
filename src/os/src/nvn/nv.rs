//! nv:: (the driver glue under NVN): the graphics layer is the host's, so it has nothing to do; the pool the game
//! hands it is recorded for the memory report.

use crate::guest::{w64, Args};
use crate::hle::Registry;
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicU64, Ordering};

static NV_POOL: AtomicU64 = AtomicU64::new(0);
static NV_SIZE: AtomicU64 = AtomicU64::new(0);
static NV_INITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe fn nv_init(c: *mut Cpu) {
    let a = Args(&mut *c);
    NV_POOL.store(a.x(0), Ordering::Relaxed);
    NV_SIZE.store(a.x(1), Ordering::Relaxed);
    NV_INITED.store(true, Ordering::Release);
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
/// nn::gs::sdk::CheckGraphicsSystemInitialized(caller, length) (Switch 2): the SDK's flag is set by
/// `BootGraphicsSystem`, which only the replaced nv::InitializeGraphics calls; the host's graphics system is up
/// once the game called nv::InitializeGraphics.
unsafe fn gs_check_initialized(c: *mut Cpu) {
    if !NV_INITED.load(Ordering::Acquire) {
        nxrt::rt::guest_fatal(c, "nn::gs::sdk::CheckGraphicsSystemInitialized before nv::InitializeGraphics");
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
    crate::log!("nv::DumpGraphicsMemoryInfo: pool {:.2} MB, the graphics layer keeps its own", NV_SIZE.load(Ordering::Relaxed) as f64 / 1048576.0);
}

pub fn register_nv(r: &mut Registry) {
    const WHY: &str = "NVN is the host's: the driver glue under it has nothing to do";
    r.force("_ZN2nv18InitializeGraphicsEPvm", WHY, nv_init);
    r.force("_ZN2nv16FinalizeGraphicsEv", WHY, nv_fin);
    r.force("_ZN2nv9InitCheckEv", WHY, nv_init_check);
    r.force("_ZN2nv21GetGraphicsMemoryInfoEPNS_10MemoryInfoE", WHY, nv_mem_info);
    r.force("_ZN2nv22DumpGraphicsMemoryInfoEv", WHY, nv_dump);
    r.force("_ZN2nn2gs3sdk30CheckGraphicsSystemInitializedEPKcm", "the gs boot is nv::InitializeGraphics' (replaced): initialized once it ran", gs_check_initialized);
}

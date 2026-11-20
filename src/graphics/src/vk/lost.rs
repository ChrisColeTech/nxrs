//! A lost device (the GPU stopped responding: a hang the driver reset, TDR, or a fault). Turbo `vk_dev.c` `vk_lost`.
//!
//! Checkpoints (VK_NV_device_diagnostic_checkpoints, `NXGFX_CHECKPOINTS`: default on for dispatches and indirect draws,
//! `all` for every draw, `0` off) mark each such command with a number; the replay keeps what each number was (shader
//! keys, sizes, the indirect arguments as recorded). On VK_ERROR_DEVICE_LOST `on_lost` logs, once, the last
//! checkpoints the GPU started and finished per queue, the commands between them, and the driver's own fault report
//! (VK_EXT_device_fault: the faulting addresses), then ends the process: nothing can run on a lost device, and
//! retrying only makes the driver reset again.

use super::Gpu;
use ash::vk;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

const N: usize = 4096;

#[derive(Clone, Default)]
struct Ck {
    seq: u64,
    what: String,
}

static RING: Mutex<Vec<Ck>> = Mutex::new(Vec::new());
static SEQ: AtomicU64 = AtomicU64::new(0);
static LOST: AtomicBool = AtomicBool::new(false);

/// 0 off, 1 dispatches and indirect draws, 2 every draw too.
pub fn level() -> u8 {
    static L: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *L.get_or_init(|| match std::env::var("NXGFX_CHECKPOINTS").as_deref() {
        Ok("0") => 0,
        Ok("all") | Ok("2") => 2,
        _ => 1,
    })
}

/// A checkpoint before a command (`direct_draw`: a plain draw, checkpointed only at level 2); `what` describes it.
pub fn checkpoint(g: &Gpu, cb: vk::CommandBuffer, direct_draw: bool, what: impl FnOnce() -> String) {
    if !g.d.caps.checkpoints || level() < if direct_draw { 2 } else { 1 } {
        return;
    }
    let s = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    {
        let mut r = RING.lock().unwrap();
        if r.is_empty() {
            r.resize(N, Ck::default());
        }
        r[s as usize % N] = Ck { seq: s, what: what() };
    }
    unsafe { g.d.ck.cmd_set_checkpoint(cb, s as usize as *const core::ffi::c_void) };
}

fn text(s: u64) -> String {
    let r = RING.lock().unwrap();
    match r.get(s as usize % N) {
        Some(c) if c.seq == s && s != 0 => format!("#{s} {}", c.what),
        _ => format!("checkpoint #{s} (no longer kept)"),
    }
}

/// Whether the device was lost (everything that waits on the GPU gives up).
pub fn is_lost() -> bool {
    LOST.load(Ordering::Relaxed)
}

/// `r` came back from the driver at `where`: on VK_ERROR_DEVICE_LOST the report, then the process ends. Other results
/// return.
pub fn check(g: &Gpu, r: vk::Result, at: &str) {
    if r == vk::Result::ERROR_DEVICE_LOST {
        on_lost(g, at);
    }
}

/// The report and the end (once; a second thread that sees the loss waits for the first to end the process).
pub fn on_lost(g: &Gpu, at: &str) -> ! {
    if LOST.swap(true, Ordering::AcqRel) {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    crate::log::red(&format!("the GPU stopped responding (device lost, first seen at {at}); what it was doing:"));
    if g.d.caps.checkpoints {
        let n = unsafe { g.d.ck.get_queue_checkpoint_data_len(g.d.queue) };
        let mut cd = vec![vk::CheckpointDataNV::default(); n.min(64)];
        if !cd.is_empty() {
            unsafe { g.d.ck.get_queue_checkpoint_data(g.d.queue, &mut cd) };
        }
        let (mut lo, mut hi) = (0u64, 0u64);
        for c in &cd {
            let s = c.p_checkpoint_marker as usize as u64;
            let at = if c.stage == vk::PipelineStageFlags::TOP_OF_PIPE {
                "started"
            } else if c.stage == vk::PipelineStageFlags::BOTTOM_OF_PIPE {
                "finished everything before"
            } else {
                "reached"
            };
            crate::log::red(&format!("  the GPU {at} {} (stage {:#x})", text(s), c.stage.as_raw()));
            if c.stage == vk::PipelineStageFlags::BOTTOM_OF_PIPE {
                lo = lo.max(s);
            } else {
                hi = hi.max(s);
            }
        }
        if cd.is_empty() {
            crate::log::red("  no checkpoint was reached");
        }
        if lo != 0 {
            for s in lo + 1..=hi.min(lo + 64) {
                crate::log::red(&format!("  in flight: {}", text(s)));
            }
        }
    } else {
        crate::log::red("  (no checkpoints: VK_NV_device_diagnostic_checkpoints is not offered, or NXGFX_CHECKPOINTS=0)");
    }
    if g.d.caps.device_fault {
        let f = g.d.fault.fp().get_device_fault_info_ext;
        let mut c = vk::DeviceFaultCountsEXT::default();
        if unsafe { f(g.dev().handle(), &mut c, core::ptr::null_mut()) } == vk::Result::SUCCESS {
            c.address_info_count = c.address_info_count.min(8);
            c.vendor_info_count = c.vendor_info_count.min(8);
            c.vendor_binary_size = 0;
            let mut ai = vec![vk::DeviceFaultAddressInfoEXT::default(); c.address_info_count as usize];
            let mut vi = vec![vk::DeviceFaultVendorInfoEXT::default(); c.vendor_info_count as usize];
            let mut fi = vk::DeviceFaultInfoEXT::default();
            fi.p_address_infos = if ai.is_empty() { core::ptr::null_mut() } else { ai.as_mut_ptr() };
            fi.p_vendor_infos = if vi.is_empty() { core::ptr::null_mut() } else { vi.as_mut_ptr() };
            let r = unsafe { f(g.dev().handle(), &mut c, &mut fi) };
            if r == vk::Result::SUCCESS || r == vk::Result::INCOMPLETE {
                let d = fi.description_as_c_str().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                crate::log::red(&format!("  the driver's fault report: {}", if d.is_empty() { "(no description)" } else { &d }));
                const TY: [&str; 7] = ["none", "invalid read", "invalid write", "invalid execute", "unknown instruction pointer", "invalid instruction pointer", "instruction pointer of the fault"];
                for a in ai.iter().take(c.address_info_count as usize) {
                    let t = TY.get(a.address_type.as_raw() as usize).copied().unwrap_or("?");
                    crate::log::red(&format!("  fault address: {t} at {:#x} (within {:#x})", a.reported_address, a.address_precision));
                }
                for v in vi.iter().take(c.vendor_info_count as usize) {
                    let d = v.description_as_c_str().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    crate::log::red(&format!("  vendor fault: {d} (code {:#x}, data {:#x})", v.vendor_fault_code, v.vendor_fault_data));
                }
            }
        } else {
            crate::log::red("  the driver has no fault report for this loss");
        }
    } else {
        crate::log::red("  (no fault report: VK_EXT_device_fault is not offered)");
    }
    crate::log::red("the device is lost: the game cannot go on; exiting (exit code 71)");
    std::thread::sleep(std::time::Duration::from_millis(200));
    std::process::exit(71)
}

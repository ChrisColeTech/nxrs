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

/// checkpoints kept (with `all` the GPU ran more than 4,096 draws behind the replay in MKW races: the faulting one was
/// gone, lane K2)
const N: usize = 1 << 18;

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
        // the pass the last started work draws into (NXGFX_CHECKPOINTS=all marks each pass's targets)
        let top = hi.max(lo);
        if top != 0 {
            let r = RING.lock().unwrap();
            for s in (top.saturating_sub(N as u64 - 1)..=top).rev() {
                if let Some(c) = r.get(s as usize % N).filter(|c| c.seq == s && c.what.starts_with("a pass")) {
                    crate::log::red(&format!("  in the pass #{s} {}", c.what));
                    break;
                }
            }
        }
        if lo != 0 {
            for s in lo + 1..=hi.min(lo + 64) {
                crate::log::red(&format!("  in flight: {}", text(s)));
            }
        }
    } else {
        crate::log::red("  (no checkpoints: VK_NV_device_diagnostic_checkpoints is not offered, or NXGFX_CHECKPOINTS=0)");
    }
    {
        let r = GONE.lock().unwrap();
        if !r.is_empty() {
            crate::log::red(&format!("  the last {} images and buffers handed to be destroyed (newest last):", r.len()));
            for (t, w) in r.iter() {
                crate::log::red(&format!("    {t:.2} s: {w}"));
            }
        }
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
                    if let Some(o) = addr_owner(a.reported_address) {
                        crate::log::red(&format!("    {o}"));
                    } else if addr_report_wanted() {
                        if let Some(n) = nearest_below(a.reported_address) {
                            crate::log::red(&format!("    the nearest bound range below it: {n}"));
                        }
                        let (l, u) = binding_counts();
                        crate::log::red(&format!("    in no memory bound or unbound since the start (the address binding report: {l} ranges bound now, {u} unbound kept)"));
                    }
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

/// The last resources handed to be destroyed (an "invalid write" fault names freed memory: is it one of these?).
static GONE: std::sync::Mutex<std::collections::VecDeque<(f64, String)>> = std::sync::Mutex::new(std::collections::VecDeque::new());

/// Notes a resource handed to be destroyed once the GPU is past it (kept: the last 48).
pub fn note_gone(what: impl FnOnce() -> String) {
    if level() == 0 {
        return;
    }
    let mut r = GONE.lock().unwrap();
    if r.len() >= 48 {
        r.pop_front();
    }
    r.push_back((crate::log::elapsed(), what()));
}

/// VK_EXT_device_address_binding_report (`NXGFX_ADDR_REPORT=1`, on with `NXGFX_CHECKPOINTS=all`): every device address
/// range bound and unbound, so a faulting address is named on a device loss (the object it is in, or the one unbound
/// there last).
pub fn addr_report_wanted() -> bool {
    match std::env::var("NXGFX_ADDR_REPORT").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => level() >= 2,
    }
}

#[derive(Clone, Copy)]
struct Bind {
    size: u64,
    ty: vk::ObjectType,
    handle: u64,
    at: f64,
}
#[derive(Default)]
struct Binds {
    live: std::collections::BTreeMap<u64, Bind>,
    /// the last binding unbound at each base address (every one since the start: a fault can name memory freed long ago)
    gone: std::collections::BTreeMap<u64, (Bind, f64)>,
}
static BINDS: Mutex<Option<Binds>> = Mutex::new(None);

/// The debug messenger's address-binding message.
pub unsafe fn on_binding(data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>) {
    if data.is_null() {
        return;
    }
    let d = &*data;
    // the binding description in the message's chain
    let mut p = d.p_next as *const vk::BaseInStructure<'_>;
    let mut b: Option<&vk::DeviceAddressBindingCallbackDataEXT<'_>> = None;
    while !p.is_null() {
        if (*p).s_type == vk::StructureType::DEVICE_ADDRESS_BINDING_CALLBACK_DATA_EXT {
            b = Some(&*(p as *const vk::DeviceAddressBindingCallbackDataEXT<'_>));
            break;
        }
        p = (*p).p_next;
    }
    let Some(b) = b else { return };
    let (ty, handle) = if d.object_count > 0 && !d.p_objects.is_null() { ((*d.p_objects).object_type, (*d.p_objects).object_handle) } else { (vk::ObjectType::UNKNOWN, 0) };
    let at = crate::log::elapsed();
    let mut g = BINDS.lock().unwrap();
    let s = g.get_or_insert_with(Binds::default);
    if b.binding_type == vk::DeviceAddressBindingTypeEXT::BIND {
        s.live.insert(b.base_address, Bind { size: b.size, ty, handle, at });
    } else if let Some(x) = s.live.remove(&b.base_address) {
        s.gone.insert(b.base_address, (x, at));
    }
}

/// Where the report saw object `handle` bound (address, size), for the loss report's pass line.
pub fn bound_at(handle: u64) -> Option<(u64, u64)> {
    let g = BINDS.lock().unwrap();
    g.as_ref()?.live.iter().find(|(_, b)| b.handle == handle).map(|(a, b)| (*a, b.size))
}

/// The live binding nearest below `a` (where its end is, against the faulting address).
fn nearest_below(a: u64) -> Option<String> {
    let g = BINDS.lock().unwrap();
    let (&base, x) = g.as_ref()?.live.range(..=a).next_back()?;
    Some(format!("{:?} {:#x} at {base:#x} +{:#x} (ends {:#x}, {:#x} before the fault)", x.ty, x.handle, x.size, base + x.size, a.saturating_sub(base + x.size)))
}

/// Bindings the report has seen: (live, unbound kept).
pub fn binding_counts() -> (usize, usize) {
    BINDS.lock().unwrap().as_ref().map(|b| (b.live.len(), b.gone.len())).unwrap_or((0, 0))
}

/// The names `Gpu::buffer` gives its buffers, by handle, while the address report is on: the report names the kind
/// of buffer a faulting address was in (staging, pool, write-back...), lane K2.
static NAMES: Mutex<Option<std::collections::HashMap<u64, String>>> = Mutex::new(None);
pub fn name_object(handle: u64, name: &str) {
    if !addr_report_wanted() {
        return;
    }
    let mut g = NAMES.lock().unwrap();
    let m = g.get_or_insert_with(std::collections::HashMap::new);
    if m.len() > 1 << 20 {
        m.clear();
    }
    m.insert(handle, name.to_string());
}
fn name_of(handle: u64) -> String {
    NAMES.lock().unwrap().as_ref().and_then(|m| m.get(&handle)).map(|n| format!(" \"{n}\"")).unwrap_or_default()
}

/// What address `a` was in: the live binding holding it, else the last binding there that was unbound.
fn addr_owner(a: u64) -> Option<String> {
    let g = BINDS.lock().unwrap();
    let s = g.as_ref()?;
    if let Some((&base, x)) = s.live.range(..=a).next_back().filter(|(&base, x)| a < base + x.size) {
        return Some(format!("inside {:?} {:#x}{} (bound at {:.2} s: {:#x} +{:#x}), still bound", x.ty, x.handle, name_of(x.handle), x.at, base, x.size));
    }
    s.gone
        .range(..=a)
        .rev()
        .find(|(base, (x, _))| a < **base + x.size)
        .map(|(base, (x, t))| format!("inside {:?} {:#x}{} (bound at {:.2} s: {:#x} +{:#x}), UNBOUND at {:.2} s", x.ty, x.handle, name_of(x.handle), x.at, base, x.size, t))
}

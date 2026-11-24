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

#[path = "lost_bindings.rs"]
pub(crate) mod bindings;

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

thread_local! {
    // The issue site samples its own recording thread, not a later checkpoint on another worker.
    static LAST_RECORDED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(crate) fn last_recorded_checkpoint() -> u64 {
    LAST_RECORDED.with(|last| last.get())
}

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
/// Returns its explicit token (0 when unavailable), so setup's intervening checkpoints cannot steal association.
pub fn checkpoint(
    g: &Gpu,
    cb: vk::CommandBuffer,
    direct_draw: bool,
    what: impl FnOnce() -> String,
) -> u64 {
    if !g.d.caps.checkpoints || level() < if direct_draw { 2 } else { 1 } {
        return 0;
    }
    let s = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    {
        let mut r = RING.lock().unwrap();
        if r.is_empty() {
            r.resize(N, Ck::default());
        }
        r[s as usize % N] = Ck {
            seq: s,
            what: what(),
        };
    }
    unsafe {
        g.d.ck
            .cmd_set_checkpoint(cb, s as usize as *const core::ffi::c_void)
    };
    LAST_RECORDED.with(|last| last.set(s));
    s
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
    crate::log::red(&format!(
        "the GPU stopped responding (device lost, first seen at {at}); what it was doing:"
    ));
    let mut binding_markers = Vec::new();
    if g.d.caps.checkpoints {
        let n = unsafe { g.d.ck.get_queue_checkpoint_data_len(g.d.queue) };
        let mut cd = vec![vk::CheckpointDataNV::default(); n.min(64)];
        if !cd.is_empty() {
            unsafe { g.d.ck.get_queue_checkpoint_data(g.d.queue, &mut cd) };
        }
        let (mut lo, mut hi) = (0u64, 0u64);
        for c in &cd {
            let s = c.p_checkpoint_marker as usize as u64;
            // Preserve every explicit marker, including bottom > top from pre/main submission order.
            if s != 0 && !binding_markers.contains(&s) {
                binding_markers.push(s);
            }
            let at = if c.stage == vk::PipelineStageFlags::TOP_OF_PIPE {
                "started"
            } else if c.stage == vk::PipelineStageFlags::BOTTOM_OF_PIPE {
                "finished everything before"
            } else {
                "reached"
            };
            crate::log::red(&format!(
                "  the GPU {at} {} (stage {:#x})",
                text(s),
                c.stage.as_raw()
            ));
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
                if let Some(c) = r
                    .get(s as usize % N)
                    .filter(|c| c.seq == s && c.what.starts_with("a pass"))
                {
                    crate::log::red(&format!("  in the pass #{s} {}", c.what));
                    break;
                }
            }
        }
        // the commands just before the last started one (a pass begins at its first draw, after that draw's checkpoint)
        if top != 0 {
            for s in top.saturating_sub(3)..top {
                crate::log::red(&format!("  before it: {}", text(s)));
            }
            if addr_report_wanted() {
                crate::log::red("  stage sets around it:");
                sets_report(top.saturating_sub(3), top + 1);
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
            crate::log::red(&format!(
                "  the last {} images and buffers handed to be destroyed (newest last):",
                r.len()
            ));
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
            let mut ai =
                vec![vk::DeviceFaultAddressInfoEXT::default(); c.address_info_count as usize];
            let mut vi =
                vec![vk::DeviceFaultVendorInfoEXT::default(); c.vendor_info_count as usize];
            let mut fi = vk::DeviceFaultInfoEXT::default();
            fi.p_address_infos = if ai.is_empty() {
                core::ptr::null_mut()
            } else {
                ai.as_mut_ptr()
            };
            fi.p_vendor_infos = if vi.is_empty() {
                core::ptr::null_mut()
            } else {
                vi.as_mut_ptr()
            };
            let r = unsafe { f(g.dev().handle(), &mut c, &mut fi) };
            if r == vk::Result::SUCCESS || r == vk::Result::INCOMPLETE {
                let d = fi
                    .description_as_c_str()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                crate::log::red(&format!(
                    "  the driver's fault report: {}",
                    if d.is_empty() { "(no description)" } else { &d }
                ));
                const TY: [&str; 7] = [
                    "none",
                    "invalid read",
                    "invalid write",
                    "invalid execute",
                    "unknown instruction pointer",
                    "invalid instruction pointer",
                    "instruction pointer of the fault",
                ];
                for a in ai.iter().take(c.address_info_count as usize) {
                    let t = TY
                        .get(a.address_type.as_raw() as usize)
                        .copied()
                        .unwrap_or("?");
                    crate::log::red(&format!(
                        "  fault address: {t} at {:#x} (within {:#x})",
                        a.reported_address, a.address_precision
                    ));
                    if a.address_type == vk::DeviceFaultAddressTypeEXT::READ_INVALID
                        || a.address_type == vk::DeviceFaultAddressTypeEXT::WRITE_INVALID
                    {
                        bindings::report(&binding_markers, a.reported_address);
                        let us = uses_of(a.reported_address);
                        crate::log::red(&format!(
                            "    {} recorded device range(s) of the replay hold it{}",
                            us.len(),
                            if us.is_empty() { "" } else { " (newest last):" }
                        ));
                        for x in us {
                            crate::log::red(&format!("      {x}"));
                        }
                    }
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
                    let d = v
                        .description_as_c_str()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    crate::log::red(&format!(
                        "  vendor fault: {d} (code {:#x}, data {:#x})",
                        v.vendor_fault_code, v.vendor_fault_data
                    ));
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
static GONE: std::sync::Mutex<std::collections::VecDeque<(f64, String)>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

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
    id: u64,
    size: u64,
    ty: vk::ObjectType,
    handle: u64,
    at: f64,
}
#[derive(Default)]
struct Binds {
    next_id: u64,
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
    let (ty, handle) = if d.object_count > 0 && !d.p_objects.is_null() {
        ((*d.p_objects).object_type, (*d.p_objects).object_handle)
    } else {
        (vk::ObjectType::UNKNOWN, 0)
    };
    let at = crate::log::elapsed();
    let mut g = BINDS.lock().unwrap();
    let s = g.get_or_insert_with(Binds::default);
    if b.binding_type == vk::DeviceAddressBindingTypeEXT::BIND {
        s.next_id += 1;
        s.live.insert(
            b.base_address,
            Bind {
                id: s.next_id,
                size: b.size,
                ty,
                handle,
                at,
            },
        );
    } else if let Some(x) = s.live.remove(&b.base_address) {
        s.gone.insert(b.base_address, (x, at));
    }
}

/// Observed driver binding generation for this buffer/address, not an allocation inferred from recycled VA.
/// None means unobserved (including disabled/unsupported address reports), not proof of a freed resource.
pub(crate) fn binding_id(handle: u64, address: u64) -> Option<u64> {
    let g = BINDS.lock().unwrap();
    g.as_ref()?
        .live
        .range(..=address)
        .rev()
        .find_map(|(&base, b)| {
            (b.ty == vk::ObjectType::BUFFER && b.handle == handle && address - base < b.size)
                .then_some(b.id)
        })
}

/// Where the report saw object `handle` bound (address, size), for the loss report's pass line.
pub fn bound_at(handle: u64) -> Option<(u64, u64)> {
    let g = BINDS.lock().unwrap();
    g.as_ref()?
        .live
        .iter()
        .find(|(_, b)| b.handle == handle)
        .map(|(a, b)| (*a, b.size))
}

/// The live binding nearest below `a` (where its end is, against the faulting address).
fn nearest_below(a: u64) -> Option<String> {
    let g = BINDS.lock().unwrap();
    let (&base, x) = g.as_ref()?.live.range(..=a).next_back()?;
    Some(format!(
        "{:?} {:#x} at {base:#x} +{:#x} (ends {:#x}, {:#x} before the fault)",
        x.ty,
        x.handle,
        x.size,
        base + x.size,
        a.saturating_sub(base + x.size)
    ))
}

/// Bindings the report has seen: (live, unbound kept).
pub fn binding_counts() -> (usize, usize) {
    BINDS
        .lock()
        .unwrap()
        .as_ref()
        .map(|b| (b.live.len(), b.gone.len()))
        .unwrap_or((0, 0))
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
/// The name a buffer was made with (address report on), if known.
pub fn buf_name(handle: u64) -> Option<String> {
    NAMES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&handle).cloned())
}
fn name_of(handle: u64) -> String {
    NAMES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&handle))
        .map(|n| format!(" \"{n}\""))
        .unwrap_or_default()
}

/// What address `a` was in: the live binding holding it, else the last binding there that was unbound; then every other
/// unbound binding that covered it (the newest owner of a recycled range is not always the one a stale reader means,
/// lane K3).
fn addr_owner(a: u64) -> Option<String> {
    let g = BINDS.lock().unwrap();
    let s = g.as_ref()?;
    let mut past: Vec<(f64, String)> = s
        .gone
        .range(..=a)
        .filter(|(base, (x, _))| a < **base + x.size)
        .map(|(base, (x, t))| {
            (
                *t,
                format!(
                    "{:?} {:#x}{} (bound at {:.2} s: {:#x} +{:#x}), UNBOUND at {:.2} s",
                    x.ty,
                    x.handle,
                    name_of(x.handle),
                    x.at,
                    base,
                    x.size,
                    t
                ),
            )
        })
        .collect();
    past.sort_by(|x, y| x.0.total_cmp(&y.0));
    let earlier = |skip_last: bool| {
        let n = past.len() - skip_last as usize;
        past[n.saturating_sub(8)..n]
            .iter()
            .map(|(_, d)| {
                format!(
                    "
      earlier: {d}"
                )
            })
            .collect::<String>()
    };
    if let Some((&base, x)) = s
        .live
        .range(..=a)
        .next_back()
        .filter(|(&base, x)| a < base + x.size)
    {
        return Some(format!(
            "inside {:?} {:#x}{} (bound at {:.2} s: {:#x} +{:#x}), still bound{}",
            x.ty,
            x.handle,
            name_of(x.handle),
            x.at,
            base,
            x.size,
            earlier(false)
        ));
    }
    let last = past.last()?.1.clone();
    Some(format!("inside {last}{}", earlier(true)))
}

/// Device ranges the replay handed the GPU (descriptor buffer / vertex / index / indirect addresses) with the checkpoint
/// they came after, at `NXGFX_CHECKPOINTS=all` with the address report: a faulting address is matched against them
/// (lane K3: MKW's 128x128x5 pass read a recycled range, and no reader was found by reading).
const UN: usize = 1 << 20;
static USES: Mutex<Vec<(u64, u64, u64, &'static str)>> = Mutex::new(Vec::new());
static USE_I: AtomicU64 = AtomicU64::new(0);
pub fn note_use(addr: u64, n: u64, what: &'static str) {
    if addr == 0 || level() < 2 || !addr_report_wanted() {
        return;
    }
    let i = USE_I.fetch_add(1, Ordering::Relaxed) as usize % UN;
    let mut u = USES.lock().unwrap();
    if u.is_empty() {
        u.resize(UN, (0, 0, 0, ""));
    }
    u[i] = (SEQ.load(Ordering::Relaxed), addr, n, what);
}
/// Stage-set events of the replay (descriptor buffers), at `NXGFX_CHECKPOINTS=all` with the address report: a set
/// written (W) or reused from the set cache (H) at (submission, chunk, offset) for a stage, a chunk bound (B).
const SN: usize = 1 << 18;
static SETS: Mutex<Vec<(u64, u8, u8, u64, u32, u64)>> = Mutex::new(Vec::new());
static SET_I: AtomicU64 = AtomicU64::new(0);
pub fn note_set(kind: u8, stage: u8, serial: u64, chunk: usize, off: u64) {
    if level() < 2 || !addr_report_wanted() {
        return;
    }
    let i = SET_I.fetch_add(1, Ordering::Relaxed) as usize % SN;
    let mut u = SETS.lock().unwrap();
    if u.is_empty() {
        u.resize(SN, (0, 0, 0, 0, 0, 0));
    }
    u[i] = (
        SEQ.load(Ordering::Relaxed),
        kind,
        stage,
        serial,
        chunk as u32,
        off,
    );
}
fn sets_report(lo: u64, hi: u64) {
    let u = SETS.lock().unwrap();
    let mut ev: Vec<(u64, u8, u8, u64, u32, u64)> = u
        .iter()
        .filter(|e| e.1 != 0 && e.0 >= lo && e.0 <= hi)
        .copied()
        .collect();
    ev.sort_by_key(|e| e.0);
    for e in ev.iter().rev().take(16).rev() {
        let what = match e.1 {
            b'W' => "set written",
            b'H' => "set reused",
            _ => "chunk bound",
        };
        let mut line = format!(
            "    after #{}: {what}: stage {} submission {} chunk {} offset {:#x}",
            e.0, e.2, e.3, e.4, e.5
        );
        if e.1 == b'H' {
            let w = u
                .iter()
                .filter(|x| x.1 == b'W' && x.3 == e.3 && x.4 == e.4 && x.5 == e.5 && x.0 <= e.0)
                .map(|x| x.0)
                .max();
            line += &match w {
                Some(s) => format!(" (written after #{s})"),
                None => " (NO write of it kept)".to_string(),
            };
        }
        crate::log::red(&line);
    }
}

fn uses_of(a: u64) -> Vec<String> {
    let u = USES.lock().unwrap();
    let mut hits: Vec<(u64, u64, u64, &'static str)> = u
        .iter()
        .filter(|e| e.2 != 0 && a >= e.1 && a < e.1 + e.2)
        .copied()
        .collect();
    hits.sort_by_key(|e| e.0);
    let n = hits.len();
    hits[n.saturating_sub(8)..]
        .iter()
        .map(|e| format!("{} {:#x} +{:#x} after {}", e.3, e.1, e.2, text(e.0)))
        .collect()
}

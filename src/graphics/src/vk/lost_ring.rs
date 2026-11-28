//! Always-on loss evidence for task / mesh draws that does not change the replay's timing: fixed-size rings of atomics
//! (no allocation, no lock on the hot path). Written: every descriptor-buffer stage set (its buffer addresses and
//! ranges), every mesh draw (its submission and the stage sets it bound), every buffer freed (its device address range,
//! the last submitted and the last completed timeline value then) and every submission's timeline value. On a loss
//! the report names the faulting draw's descriptor holding the faulting address and whether that memory was freed,
//! and when against the draw's submission (MKW-LOSS: the heavier recorders hid the fault).
use std::sync::atomic::{AtomicU64, Ordering::{Acquire, Relaxed, Release}};

/// Bindings kept per stage set.
pub const MAXB: usize = 8;
const SET_N: usize = 16384;
/// sequence, serial, chunk << 32 | stage, offset, shader, count, 4 words a binding, the chunk's device address
const SET_W: usize = 7 + 4 * MAXB;
const DRAW_N: usize = 1024;
const DRAW_W: usize = 12;
const FREE_N: usize = 65536;
const FREE_W: usize = 8;
const SUB_N: usize = 2048;
const SUB_W: usize = 4;

static SETS: [[AtomicU64; SET_W]; SET_N] = [const { [const { AtomicU64::new(0) }; SET_W] }; SET_N];
static SET_I: AtomicU64 = AtomicU64::new(0);
static DRAWS: [[AtomicU64; DRAW_W]; DRAW_N] =
    [const { [const { AtomicU64::new(0) }; DRAW_W] }; DRAW_N];
static DRAW_I: AtomicU64 = AtomicU64::new(0);
static FREES: [[AtomicU64; FREE_W]; FREE_N] =
    [const { [const { AtomicU64::new(0) }; FREE_W] }; FREE_N];
static FREE_I: AtomicU64 = AtomicU64::new(0);
static SUBS: [[AtomicU64; SUB_W]; SUB_N] = [const { [const { AtomicU64::new(0) }; SUB_W] }; SUB_N];
static SUB_I: AtomicU64 = AtomicU64::new(0);
/// The last timeline value the completion thread saw complete.
pub static COMPLETED: AtomicU64 = AtomicU64::new(0);

/// Microseconds since the start.
fn now_us() -> u64 {
    (crate::log::elapsed() * 1e6) as u64
}

/// One slot written: its sequence word cleared, the fields, then the sequence (a reader drops a slot whose sequence
/// changed while it read).
fn put<const W: usize>(ring: &[[AtomicU64; W]], i: &AtomicU64, fields: &[u64]) {
    let s = i.fetch_add(1, Relaxed) + 1;
    let slot = &ring[s as usize % ring.len()];
    slot[0].store(0, Release);
    for (w, v) in slot[1..].iter().zip(fields) {
        w.store(*v, Relaxed);
    }
    slot[0].store(s, Release);
}

/// The slots holding a whole record, oldest first.
fn read<const W: usize>(ring: &[[AtomicU64; W]], i: &AtomicU64) -> Vec<[u64; W]> {
    let top = i.load(Acquire);
    let lo = top.saturating_sub(ring.len() as u64 - 1).max(1);
    let mut out = Vec::new();
    for s in lo..=top {
        let slot = &ring[s as usize % ring.len()];
        if slot[0].load(Acquire) != s {
            continue;
        }
        let mut r = [0u64; W];
        for (k, w) in slot.iter().enumerate() {
            r[k] = w.load(Relaxed);
        }
        if slot[0].load(Acquire) == s {
            out.push(r);
        }
    }
    out
}

/// A stage set written into a descriptor buffer: (binding, descriptor type, device address, range, buffer handle).
pub fn note_set(
    serial: u64,
    chunk: usize,
    chunk_addr: u64,
    off: u64,
    stage: u32,
    key: [u8; 8],
    b: &[(u32, i32, u64, u64, u64)],
) {
    let mut f = [0u64; SET_W - 1];
    f[SET_W - 2] = chunk_addr;
    f[0] = serial;
    f[1] = (chunk as u64) << 32 | stage as u64;
    f[2] = off;
    f[3] = u64::from_le_bytes(key);
    f[4] = b.len().min(MAXB) as u64;
    for (k, x) in b.iter().take(MAXB).enumerate() {
        f[5 + 4 * k] = (x.0 as u64) << 32 | x.1 as u32 as u64;
        f[6 + 4 * k] = x.2;
        f[7 + 4 * k] = x.3;
        f[8 + 4 * k] = x.4;
    }
    put(&SETS, &SET_I, &f);
}

/// A mesh draw recorded: its checkpoint, submission, the descriptor buffer chunk's address and the (chunk, offset) of
/// the stage sets of stages 1, 3 and 4 (u64::MAX: none).
pub fn note_draw(checkpoint: u64, serial: u64, chunk_addr: u64, sets: [(u64, u64); 3]) {
    let mut f = [0u64; DRAW_W - 1];
    f[0] = checkpoint;
    f[1] = serial;
    f[2] = now_us();
    f[3] = chunk_addr;
    for (k, (c, o)) in sets.iter().enumerate() {
        f[4 + 2 * k] = *c;
        f[5 + 2 * k] = *o;
    }
    put(&DRAWS, &DRAW_I, &f);
}

const DISP_N: usize = 1024;
const DISP_W: usize = 10;
static DISPS: [[AtomicU64; DISP_W]; DISP_N] = [const { [const { AtomicU64::new(0) }; DISP_W] }; DISP_N];
static DISP_I: AtomicU64 = AtomicU64::new(0);

/// Where a dispatch (or draw) read its descriptors from, as bound when it was recorded (OOT-LOAD).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DescBinding {
    /// the stage-set chunk bound (device address, size)
    pub chunk: (u64, u64),
    /// the set's offset in that chunk and the set's size
    pub set: (u64, u64),
    /// the pool set's buffer bound (device address, size; 0 when the work does not read the pool set)
    pub pool: (u64, u64),
}

/// A dispatch recorded: its checkpoint, submission and the descriptor buffers it reads (`DescBinding`).
pub fn note_dispatch(checkpoint: u64, serial: u64, b: DescBinding) {
    put(
        &DISPS,
        &DISP_I,
        &[checkpoint, serial, now_us(), b.chunk.0, b.chunk.1, b.set.0, b.set.1, b.pool.0, b.pool.1],
    );
}

/// Where `fault` lies against [a, a + n): inside, past the end or below the start (by how much).
pub fn relation(fault: u64, a: u64, n: u64) -> String {
    if a == 0 {
        "none".to_string()
    } else if fault < a {
        format!("{:#x} below its start", a - fault)
    } else if fault < a + n {
        format!("INSIDE it at +{:#x}", fault - a)
    } else {
        format!("{:#x} past its end", fault - (a + n))
    }
}

/// The report lines for the dispatches among `markers`: the buffers each read its descriptors from, and where the
/// faulting page lies against them.
fn dispatch_report(markers: &[u64], fault: u64, sets: &[[u64; SET_W]], out: &mut Vec<String>) {
    let ds = read(&DISPS, &DISP_I);
    let subs = read(&SUBS, &SUB_I);
    for &mk in markers {
        let Some(d) = ds.iter().rev().find(|d| d[1] == mk) else {
            continue;
        };
        let (serial, chunk, chunk_n, off, set_n, pool, pool_n) = (d[2], d[4], d[5], d[6], d[7], d[8], d[9]);
        let value = match subs.iter().rev().find(|s| s[1] == serial) {
            Some(s) => format!("timeline {} submitted at {:.3} s", s[2], s[3] as f64 / 1e6),
            None => "its submission is not kept".to_string(),
        };
        out.push(format!(
            "dispatch #{mk}: recorded at {:.3} s in submission {serial} ({value}); stage-set chunk {chunk:#x} +{chunk_n:#x} (the fault {}), its set at +{off:#x} ({set_n} bytes: the fault {}), pool set buffer {pool:#x} +{pool_n:#x} (the fault {})",
            d[3] as f64 / 1e6,
            relation(fault, chunk, chunk_n),
            relation(fault, chunk + off, set_n),
            relation(fault, pool, pool_n),
        ));
        if off + set_n > chunk_n {
            out.push("  its set ENDS PAST its chunk".to_string());
        }
        if let Some(s) = sets.iter().rev().find(|s| s[1] == serial && s[3] == off && s[SET_W - 1] == chunk) {
            for b in 0..(s[5] as usize).min(MAXB) {
                let (bk, addr, range) = (s[6 + 4 * b], s[7 + 4 * b], s[8 + 4 * b]);
                out.push(format!(
                    "  binding {} (type {}): {addr:#x} +{range:#x} (the fault {})",
                    bk >> 32,
                    bk as u32 as i32,
                    relation(fault, addr, range)
                ));
            }
        } else {
            out.push("  its set's writing is not kept".to_string());
        }
    }
}

/// A buffer freed (`kind` 1: its memory goes now; 2: a pool's buffer handed to be freed after use; 3: a descriptor
/// buffer's memory goes now).
pub fn note_free(kind: u64, addr: u64, size: u64, handle: u64, last_submitted: u64) {
    note_mem(kind, addr, size, handle, last_submitted, "");
}

/// A buffer made (`kind` 11: a buffer with its memory, named `name`; 13: a descriptor buffer) or freed (`note_free`).
pub fn note_mem(kind: u64, addr: u64, size: u64, handle: u64, last_submitted: u64, name: &str) {
    if addr == 0 {
        return;
    }
    let mut n = [0u8; 8];
    for (d, s) in n.iter_mut().zip(name.bytes()) {
        *d = s;
    }
    put(
        &FREES,
        &FREE_I,
        &[
            kind,
            addr,
            size,
            handle,
            now_us(),
            last_submitted << 32 | COMPLETED.load(Relaxed) & 0xffff_ffff,
            u64::from_le_bytes(n),
        ],
    );
}

const USE_N: usize = 16384;
const USE_W: usize = 7;
static USES: [[AtomicU64; USE_W]; USE_N] = [const { [const { AtomicU64::new(0) }; USE_W] }; USE_N];
static USE_I: AtomicU64 = AtomicU64::new(0);

/// A buffer range handed to the GPU outside a descriptor (vertex, index, indirect) in recording `serial`.
pub fn note_use(serial: u64, handle: u64, off: u64, n: u64, what: &'static str) {
    put(&USES, &USE_I, &[serial, handle, off, n, what.as_ptr() as u64, what.len() as u64]);
}

/// The stage sets and other uses of buffer `handle` kept, newest last: (serial, description).
pub fn uses_of_handle(handle: u64) -> Vec<(u64, String)> {
    let mut out: Vec<(u64, String)> = Vec::new();
    for s in read(&SETS, &SET_I) {
        for b in 0..(s[5] as usize).min(MAXB) {
            if s[9 + 4 * b] == handle {
                out.push((
                    s[1],
                    format!(
                        "submission {}: stage {} set at {:#x} (chunk {} at {:#x} + {:#x}) (shader {:016x}) binding {} at {:#x} +{:#x}",
                        s[1],
                        s[2] & 0xffff_ffff,
                        s[SET_W - 1] + s[3],
                        s[2] >> 32,
                        s[SET_W - 1],
                        s[3],
                        u64::from_be_bytes(s[4].to_le_bytes()),
                        s[6 + 4 * b] >> 32,
                        s[7 + 4 * b],
                        s[8 + 4 * b]
                    ),
                ));
            }
        }
    }
    for u in read(&USES, &USE_I) {
        if u[2] == handle {
            // SAFETY: a &'static str's pointer and length, written together in one record
            let what = unsafe {
                std::str::from_utf8_unchecked(core::slice::from_raw_parts(u[5] as *const u8, u[6] as usize))
            };
            out.push((u[1], format!("submission {}: {what} at +{:#x} ({:#x} bytes)", u[1], u[3], u[4])));
        }
    }
    out.sort_by_key(|x| x.0);
    out
}

/// A submission submitted: its recording serial and timeline value.
pub fn note_submit(serial: u64, value: u64) {
    put(&SUBS, &SUB_I, &[serial, value, now_us()]);
}

/// The record of a free, for tests and the report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Free {
    pub kind: u64,
    pub addr: u64,
    pub size: u64,
    pub handle: u64,
    pub at_us: u64,
    pub last_submitted: u64,
    pub completed: u64,
    pub name: [u8; 8],
}

/// Frees whose range holds [a, a + n) partly or wholly, oldest first.
pub fn frees_over(a: u64, n: u64) -> Vec<Free> {
    mem_over(a, n).into_iter().filter(|f| f.kind < 10).collect()
}

/// Buffers made and freed over [a, a + n), oldest first.
pub fn mem_over(a: u64, n: u64) -> Vec<Free> {
    read(&FREES, &FREE_I)
        .into_iter()
        .filter(|r| r[2] != 0 && a < r[2] + r[3] && r[2] < a + n.max(1))
        .map(|r| Free {
            name: r[7].to_le_bytes(),
            kind: r[1],
            addr: r[2],
            size: r[3],
            handle: r[4],
            at_us: r[5],
            last_submitted: r[6] >> 32,
            completed: r[6] & 0xffff_ffff,
        })
        .collect()
}

/// The report lines for a loss: the draws among `markers` (checkpoints the GPU reported) with their stage sets, the
/// descriptor holding `fault` and the frees over each bound range.
pub fn report(markers: &[u64], fault: u64) -> Vec<String> {
    let mut out = Vec::new();
    let draws = read(&DRAWS, &DRAW_I);
    let sets = read(&SETS, &SET_I);
    let subs = read(&SUBS, &SUB_I);
    let mut ms: Vec<u64> = markers.to_vec();
    ms.sort_unstable();
    ms.dedup();
    dispatch_report(&ms, fault, &sets, &mut out);
    for &mk in &ms {
        let Some(d) = draws.iter().rev().find(|d| d[1] == mk) else {
            continue;
        };
        let serial = d[2];
        let sub = subs.iter().rev().find(|s| s[1] == serial);
        out.push(format!(
            "mesh draw #{mk}: recorded at {:.3} s in submission {serial} ({}), descriptor buffer bound at {:#x}",
            d[3] as f64 / 1e6,
            match sub {
                Some(s) => format!("timeline {} submitted at {:.3} s", s[2], s[3] as f64 / 1e6),
                None => "its submission is not kept".to_string(),
            },
            d[4]
        ));
        // the stage sets written after this draw's in its submission (the work after it that has no checkpoint)
        let (c3, o3) = (d[7], d[8]);
        let after: Vec<&[u64; SET_W]> = sets
            .iter()
            .filter(|s| s[1] == serial && s[2] >> 32 == c3 && s[3] > o3)
            .collect();
        out.push(format!("  stage sets written after it in its submission: {}", after.len()));
        for s in after.iter().take(24) {
            let mut l = format!(
                "    chunk {} offset {:#x} stage {} shader {:016x}:",
                s[2] >> 32,
                s[3],
                s[2] & 0xffff_ffff,
                u64::from_be_bytes(s[4].to_le_bytes())
            );
            for b in 0..(s[5] as usize).min(MAXB) {
                l += &format!(" b{} {:#x}+{:#x}", s[6 + 4 * b] >> 32, s[7 + 4 * b], s[8 + 4 * b]);
            }
            out.push(l);
        }
        for (k, stage) in [1u64, 3, 4].iter().enumerate() {
            let (chunk, off) = (d[5 + 2 * k], d[6 + 2 * k]);
            if chunk == u64::MAX {
                continue;
            }
            let Some(s) = sets
                .iter()
                .rev()
                .find(|s| s[1] == serial && s[2] == (chunk << 32 | stage) && s[3] == off)
            else {
                out.push(format!("  stage {stage}: set at chunk {chunk} offset {off:#x}: its writing is not kept"));
                continue;
            };
            out.push(format!(
                "  stage {stage}: shader {:016x}, set at chunk {chunk} ({:#x}) offset {off:#x}",
                u64::from_be_bytes(s[4].to_le_bytes()),
                s[SET_W - 1]
            ));
            for b in 0..(s[5] as usize).min(MAXB) {
                let (bk, addr, range, handle) = (s[6 + 4 * b], s[7 + 4 * b], s[8 + 4 * b], s[9 + 4 * b]);
                let holds = fault >= addr && fault < addr.saturating_add(range);
                out.push(format!(
                    "    binding {} (type {}): {addr:#x} +{range:#x} buffer {handle:#x}{}",
                    bk >> 32,
                    bk as u32 as i32,
                    if holds { "  <- HOLDS THE FAULTING ADDRESS" } else { "" }
                ));
                for f in frees_over(addr, range) {
                    out.push(format!("      {}", free_line(&f, sub.map(|s| s[2]))));
                }
            }
        }
    }
    // every kept stage set with a binding over the faulting page (an older submission's, when a stale descriptor is read)
    let page = fault & !0xfff;
    let mut over = 0;
    for s in &sets {
        for b in 0..(s[5] as usize).min(MAXB) {
            let (a, n) = (s[7 + 4 * b], s[8 + 4 * b]);
            if a < page + 0x1000 && page < a.saturating_add(n) {
                over += 1;
                if over <= 16 {
                    out.push(format!(
                        "set over the faulting page: submission {} stage {} shader {:016x} at chunk {} ({:#x}) offset {:#x}: binding {} {a:#x} +{n:#x}",
                        s[1],
                        s[2] & 0xffff_ffff,
                        u64::from_be_bytes(s[4].to_le_bytes()),
                        s[2] >> 32,
                        s[SET_W - 1],
                        s[3],
                        s[6 + 4 * b] >> 32
                    ));
                }
            }
        }
    }
    out.push(format!("kept stage sets with a binding over the faulting page: {over}"));
    let fs = mem_over(fault, 1);
    out.push(format!("buffers made and freed over the faulting address {fault:#x}: {}", fs.len()));
    for f in fs.iter().rev().take(12).rev() {
        out.push(format!("  {}", free_line(f, None)));
    }
    // what lay just below it (a read running past the end of a buffer)
    let below: Vec<Free> = mem_over(fault.saturating_sub(64 << 20), 64 << 20)
        .into_iter()
        .filter(|f| f.addr + f.size <= fault)
        .collect();
    let mut near = below.clone();
    near.sort_by_key(|f| std::cmp::Reverse(f.addr + f.size));
    out.push(format!("  ending within 64 MB below it: {}", below.len()));
    for f in near.iter().take(10) {
        out.push(format!("    {} (ends {:#x} below)", free_line(f, None), fault - (f.addr + f.size)));
    }
    // and just above it (a read running below the start of a buffer: a negative index)
    let mut above: Vec<Free> = mem_over(fault + 1, 64 << 20).into_iter().filter(|f| f.addr > fault).collect();
    above.sort_by_key(|f| f.addr);
    out.push(format!("  starting within 64 MB above it: {}", above.len()));
    for f in above.iter().take(10) {
        out.push(format!("    {} (starts {:#x} above)", free_line(f, None), f.addr - fault));
    }
    let mut handles: Vec<u64> = fs.iter().rev().take(4).map(|f| f.handle).collect();
    handles.dedup();
    for h in handles {
        let us = uses_of_handle(h);
        out.push(format!("  uses of buffer {h:#x} kept: {}", us.len()));
        for (_, l) in us.iter().rev().take(12).rev() {
            out.push(format!("    {l}"));
        }
    }
    out
}

fn free_line(f: &Free, draw_value: Option<u64>) -> String {
    let when = match draw_value {
        Some(v) if f.completed < v && f.last_submitted < v => " BEFORE the draw's submission was submitted",
        Some(v) if f.completed < v => " BEFORE the draw's submission completed",
        Some(_) => " after the draw's submission completed",
        None => "",
    };
    let name = String::from_utf8_lossy(&f.name).trim_end_matches('\0').to_string();
    format!(
        "{} {:#x} +{:#x} (buffer {:#x}) at {:.3} s (timeline submitted {}, completed {}){when}",
        match f.kind {
            2 => "pool buffer handed to be freed:".to_string(),
            3 => "descriptor buffer freed:".to_string(),
            11 => format!("made \"{name}\":"),
            13 => "descriptor buffer made:".to_string(),
            _ => "freed:".to_string(),
        },
        f.addr,
        f.size,
        f.handle,
        f.at_us as f64 / 1e6,
        f.last_submitted,
        f.completed
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_freed_range_is_found_by_any_address_inside_it_and_only_there() {
        note_free(1, 0x7_0000_0000, 0x2000, 0xabc, 5);
        let hit = frees_over(0x7_0000_1ff0, 4);
        assert!(hit.iter().any(|f| f.addr == 0x7_0000_0000 && f.handle == 0xabc && f.last_submitted == 5));
        assert!(frees_over(0x7_0000_2000, 4).iter().all(|f| f.addr != 0x7_0000_0000));
    }

    #[test]
    fn the_report_names_the_binding_holding_the_fault_and_its_free() {
        let ck = 0x5eed_0001;
        note_set(77, 2, 0xdb00_0000, 0x300, 3, [1, 2, 3, 4, 5, 6, 7, 8], &[(1, 6, 0x9_0000_0100, 0x100, 0x11), (3, 7, 0x9_1000_0000, 0x3440, 0x22)]);
        note_draw(ck, 77, 0xdead_0000, [(u64::MAX, 0), (2, 0x300), (u64::MAX, 0)]);
        note_submit(77, 40);
        note_free(1, 0x9_1000_0000, 0x4000, 0x22, 39);
        let r = report(&[ck], 0x9_1000_1000).join("\n");
        assert!(r.contains("submission 77 (timeline 40"), "{r}");
        assert!(r.contains("binding 3 (type 7): 0x910000000 +0x3440 buffer 0x22  <- HOLDS THE FAULTING ADDRESS"), "{r}");
        assert!(r.contains("BEFORE the draw's submission was submitted"), "{r}");
        assert!(!r.contains("binding 1 (type 6): 0x900000100 +0x100 buffer 0x11  <- HOLDS"), "{r}");
    }

    #[test]
    fn a_dispatch_report_places_the_fault_against_its_descriptor_buffers_and_names_buffers_above() {
        let ck = 0x5eed_0002;
        let b = DescBinding { chunk: (0xa_0000_0000, 0x40_0000), set: (0x3f_f000, 1600), pool: (0xa_0041_0000, 0xa1_0000) };
        note_dispatch(ck, 91, b);
        note_mem(13, 0xa_0041_0000, 0xa1_0000, 0x33, 0, "");
        let r = report(&[ck], 0xa_0040_1000).join("\n");
        assert!(r.contains("dispatch #"), "{r}");
        assert!(r.contains("(the fault 0x1000 past its end)"), "{r}");
        assert!(r.contains("(the fault 0xf000 below its start)"), "{r}");
        assert!(r.contains("starts 0xf000 above"), "{r}");
        assert_eq!(relation(5, 4, 2), "INSIDE it at +0x1");
    }
}

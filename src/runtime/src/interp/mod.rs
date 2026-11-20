//! The interpreter: guest code the build did not translate, run instruction by instruction on the same `Cpu`, guest
//! memory, SVC entry and host-call boundary as the lifted code, back in lifted code at the first known function start
//! it reaches.
//!
//! **When it runs.** `rt_dispatch` finds no lifted function at an aligned address inside a module, and no live setjmp
//! owner holds it (`dispatch::nxrt_dispatch_resolve`): before, a fatal miss. With `NXRS_INTERP_ONLY=1`, or a build
//! without any lifted function (`nxrs-build --interp-only`), every dispatch goes here (a diagnostic: is a fault the
//! translator's?). Addresses outside every module and misaligned ones stay fatal.
//!
//! **Semantics.** One decode and one meaning with the translator: `exec` follows the lifter's decode
//! (`src/translator/src/xlate/lifter.rs`, its bit helpers are that file's own `util.rs`), `simd` the FP / SIMD lowering
//! (`xlate/simd.rs`) and the C helpers it calls (`runtime/simd.h`, `simd_fp.h`; their tables are read from simd.h). An
//! encoding the lifter refuses ends the run here as it would in lifted code. The T-lane oracle (`src/oracle`,
//! `--interp`) checks the two against dynarmic class by class.
//!
//! **Calls and returns** keep the lifted code's host-stack model: BL / BLR call through `nxrt_call_guest` (a barrier,
//! so a guest longjmp past the interpreter returns through it), RET returns to whoever entered the interpreter. A
//! jump to a known function start, a host function or out of the modules leaves through `rt_dispatch` (a tail jump from
//! the asm shim, no Rust frame left behind). The interpreter runs under an `rt_sj_owner` frame, so a non-local jump
//! back into it returns to that frame: a longjmp to a setjmp call in code the build missed, or an unwinder's resume at
//! a landing pad there. The interpreter does not know a function's extent, so ownership is decided per jump (`owns`):
//! never for a call (the caller's BL / BLR names the target, or a host -> guest call), else for the return site of a
//! call the frame made (setjmp returning again) or an address in the same function (by the module's
//! `.eh_frame_hdr`) as the call the frame is in (a landing pad). A register branch in interpreted code asks the same
//! owner search, so a resume from interpreted code reaches the frame below it too.
//!
//! **Execution hints.** Every address the interpreter is entered at is appended to `<data>\discovered.tsv` (module,
//! offset, and the first four instruction words there as a stamp); `nxrs-build` adds those as function starts at the
//! next build of the same program, so they are translated from then on (Turbo's `.hints`; PORT_PLAN 3.1).

pub mod exec;
pub mod fp;
pub mod funcs;
pub mod lanes;
pub mod mem;
pub mod simd;
pub mod tables;

/// The lifter's own bit helpers (`bits`, `sext`, `ones`, `decode_bit_masks`): one definition for both.
#[path = "../../../translator/src/xlate/util.rs"]
#[allow(dead_code)]
pub mod xutil;

use crate::asm::{ST_JUMP, ST_REDISPATCH, ST_RETURN};
use crate::cpu::{Cpu, GuestFn};
use exec::Flow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static ONLY: AtomicBool = AtomicBool::new(false);
static ENTRIES: AtomicU64 = AtomicU64::new(0);
static STEPS: AtomicU64 = AtomicU64::new(0);

/// Every dispatch interpreted: `NXRS_INTERP_ONLY=1`, or a build without lifted functions (`set_only`).
pub fn only() -> bool {
    ONLY.load(Ordering::Relaxed)
        || *ENV_ONLY.get_or_init(|| std::env::var("NXRS_INTERP_ONLY").is_ok_and(|v| !v.is_empty() && v != "0"))
}
static ENV_ONLY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn set_only(on: bool) {
    ONLY.store(on, Ordering::Relaxed);
}

/// (times the interpreter was entered, instructions it ran) since start.
pub fn stats() -> (u64, u64) {
    (ENTRIES.load(Ordering::Relaxed), STEPS.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------- discovered.tsv

pub const DISCOVERED: &str = "discovered.tsv";

struct Discovered {
    path: Option<PathBuf>,
    /// (module, offset) of every entry in the file or recorded this run.
    seen: HashSet<(String, u64)>,
    new: usize,
}

static DISC: Mutex<Option<Discovered>> = Mutex::new(None);

/// Where `discovered.tsv` goes: the game's data folder (`<name>_data`). Without it nothing is written (unit tests,
/// the dev tool). Entries already in the file are not appended again.
pub fn set_data_dir(dir: &std::path::Path) {
    let path = dir.join(DISCOVERED);
    let mut seen = HashSet::new();
    if let Ok(t) = std::fs::read_to_string(&path) {
        seen.extend(parse_discovered(&t).into_iter().map(|e| (e.module, e.offset)));
    }
    *DISC.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Discovered { path: Some(path), seen, new: 0 });
}

/// One line of discovered.tsv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub module: String,
    pub offset: u64,
    /// The first instruction words at the entry, as the running game had them: the next build takes the entry only
    /// when its module has the same words there (an entry of another version of the program is left out).
    pub words: Vec<u32>,
}

/// The entries of a discovered.tsv text (comments and malformed lines skipped).
pub fn parse_discovered(text: &str) -> Vec<Entry> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut f = l.split('\t');
            let module = f.next()?.trim().to_string();
            let offset = u64::from_str_radix(f.next()?.trim().trim_start_matches("0x"), 16).ok()?;
            let words = f.next().unwrap_or("").split_whitespace().filter_map(|w| u32::from_str_radix(w, 16).ok()).collect();
            Some(Entry { module, offset, words })
        })
        .collect()
}

pub fn format_entry(e: &Entry) -> String {
    let w: Vec<String> = e.words.iter().map(|w| format!("{w:08x}")).collect();
    format!("{}\t{:#x}\t{}\n", e.module, e.offset, w.join(" "))
}

const HEADER: &str = "# nxrs discovered.tsv: code the build missed and the interpreter ran. nxrs-build adds each entry as a \
function start at the next build (when the module still has these instruction words there).\n# module\toffset\twords\n";

/// Records an interpreter entry at `pc` (once per address).
fn record(pc: u64) {
    if only() {
        return;
    }
    let Some(m) = crate::dispatch::table().and_then(|t| t.module_at(pc)) else { return };
    let mut g = DISC.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let d = g.get_or_insert_with(|| Discovered { path: None, seen: HashSet::new(), new: 0 });
    if !d.seen.insert((m.name.clone(), pc - m.base)) {
        return;
    }
    d.new += 1;
    let n = (((m.base + m.size).saturating_sub(pc)) / 4).min(4);
    // SAFETY: inside a mapped module
    let words: Vec<u32> = (0..n).map(|i| unsafe { core::ptr::read_unaligned((pc + 4 * i) as *const u32) }).collect();
    let e = Entry { module: m.name.clone(), offset: pc - m.base, words };
    if d.new <= 32 || crate::rt::trace("interp") {
        crate::rt::log(&format!(
            "interp: {} was not translated; interpreting it{}",
            crate::dispatch::guest_name(pc),
            if d.path.is_some() { " (recorded in discovered.tsv: the next build translates it)" } else { "" }
        ));
    }
    if let Some(p) = &d.path {
        let fresh = !p.exists();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let mut s = String::new();
            if fresh {
                s.push_str(HEADER);
            }
            s.push_str(&format_entry(&e));
            let _ = f.write_all(s.as_bytes());
        }
    }
}

/// How many entries this run added to discovered.tsv.
pub fn discovered_this_run() -> usize {
    DISC.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().map_or(0, |d| d.new)
}

// ---------------------------------------------------------------- dispatch

/// For `nxrt_dispatch_resolve`: the interpreter's shim when it takes `pc` (an aligned address inside a module), after
/// recording the entry.
pub fn take(pc: u64) -> Option<GuestFn> {
    if pc & 3 != 0 {
        return None;
    }
    crate::dispatch::table()?.module_at(pc)?;
    ENTRIES.fetch_add(1, Ordering::Relaxed);
    record(pc);
    Some(nxrt_interp_owner as GuestFn)
}

/// Where control leaves the interpreter for `rt_dispatch`: a lifted function starts at `pc`, a host function is there,
/// or `pc` is outside every module (or misaligned).
/// `jumped`: control did not just fall through (the aliases of patch sites are looked up only then: a lock).
fn hand_off(pc: u64, jumped: bool) -> bool {
    if pc & 3 != 0 {
        return true;
    }
    let Some(t) = crate::dispatch::table() else { return true };
    if !only() && t.lookup(pc).is_some() {
        return true;
    }
    if t.module_at(pc).is_none() || crate::hostfn::lookup(pc).is_some() {
        return true;
    }
    jumped && crate::hostfn::aliased(pc).is_some()
}

/// The end of the interpreted range starting at `c->pc`: the next known function start of its module, else the module's
/// end (`nxrt_interp_owner` hands it to `rt_sj_owner`).
#[no_mangle]
pub unsafe extern "C" fn nxrt_interp_span_end(c: *mut Cpu) -> u64 {
    let pc = (*c).pc;
    let Some(m) = crate::dispatch::table().and_then(|t| t.module_at(pc)) else { return pc + 4 };
    let k = m.addrs.partition_point(|&a| a <= pc);
    if only() {
        return m.base + m.size;
    }
    m.addrs.get(k).copied().unwrap_or(m.base + m.size)
}

// ---------------------------------------------------------------- interpreter frames as setjmp owners

/// A live interpreter frame: its owner slot (`rt_sj_owner`), the return sites of the calls it made and the
/// address of the call it is in (or made last).
struct Frame {
    slot: usize,
    sites: Vec<u64>,
    active: u64,
}

thread_local! {
    static FRAMES: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

/// Whether the owner slot `s` is an interpreter frame's.
pub fn is_slot(s: &crate::asm::SjSlot) -> bool {
    s.body == nxrt_interp_entry as *const () as usize
}

/// Whether a dispatch to `c.pc` is a call: x30 is the host-return address (a host -> guest call), or the instruction
/// before x30 is a BL to `c.pc` or a BLR through a register holding it.
///
/// # Safety
/// `c` is the calling thread's guest state.
pub unsafe fn is_call(c: &Cpu) -> bool {
    let (pc, r) = (c.pc, c.x[30]);
    if r == crate::hostfn::RET_HOST {
        return true;
    }
    if r & 3 != 0 || r < 4 || crate::dispatch::table().and_then(|t| t.module_at(r - 4)).is_none() {
        return false;
    }
    let w = core::ptr::read_unaligned((r - 4) as *const u32);
    if w & 0xFC00_0000 == 0x9400_0000 {
        let off = (((w & 0x03FF_FFFF) << 6) as i32 >> 6) as i64 * 4;
        return (r - 4).wrapping_add(off as u64) == pc;
    }
    if w & 0xFFFF_FC1F == 0xD63F_0000 {
        let n = ((w >> 5) & 31) as usize;
        return n != 30 && n != 31 && c.x[n] == pc;
    }
    false
}

/// Whether the interpreter frame of slot `s` takes the jump to `c.pc` (see the module header).
///
/// # Safety
/// `c` is the calling thread's guest state.
pub unsafe fn owns(s: &crate::asm::SjSlot, c: &Cpu) -> bool {
    let (slot, pc) = (s as *const _ as usize, c.pc);
    if is_call(c) {
        return false;
    }
    let active = FRAMES.with(|f| {
        f.borrow().iter().find(|fr| fr.slot == slot).map(|fr| (fr.sites.contains(&pc), fr.active))
    });
    match active {
        Some((true, _)) => true,
        Some((false, call)) if call != 0 => funcs::function_of(call).is_some_and(|f| funcs::function_of(pc) == Some(f)),
        _ => false,
    }
}

/// Registers the running frame (its slot is `c.sj`); a first entry (at the slot's own start) starts afresh, a resume
/// keeps the frame's sites. Frames whose slots left the chain are dropped.
unsafe fn frame_enter(c: &Cpu) -> usize {
    let slot = c.sj as usize;
    let first = slot == 0 || (*(slot as *const crate::asm::SjSlot)).lo == c.pc;
    let mut live = vec![];
    let mut s = c.sj as *const crate::asm::SjSlot;
    while !s.is_null() {
        live.push(s as usize);
        s = (*s).up;
    }
    FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        f.retain(|fr| live.contains(&fr.slot) && !(first && fr.slot == slot));
        if !f.iter().any(|fr| fr.slot == slot) {
            f.push(Frame { slot, sites: vec![], active: 0 });
        }
    });
    slot
}

fn add_site(slot: usize, call: u64) {
    FRAMES.with(|f| {
        if let Some(fr) = f.borrow_mut().iter_mut().rev().find(|fr| fr.slot == slot) {
            fr.active = call;
            let site = call.wrapping_add(4);
            if !fr.sites.contains(&site) {
                fr.sites.push(site);
            }
        }
    });
}

/// Called by `nxrt_interp_entry` (asm) for `c->pc`; returns a shim status (`asm::ST_*`).
#[no_mangle]
pub unsafe extern "C" fn nxrt_interp_invoke(c: *mut Cpu) -> u32 {
    crate::boundary::guard("the interpreter", || run(&mut *c))
}

/// Interprets from `c.pc` until the code returns (RET: `ST_RETURN`), leaves for `rt_dispatch` (`ST_REDISPATCH`, `c.pc`
/// set) or a non-local jump left a call (`ST_REDISPATCH` / `ST_JUMP`).
///
/// # Safety
/// `c` is the calling thread's guest state, `c.pc` inside a module.
pub unsafe fn run(c: &mut Cpu) -> u32 {
    let mut pc = c.pc;
    let slot = frame_enter(c);
    let mut first = true;
    let mut steps = 0u64;
    let mut next = pc;
    let status = loop {
        if !first && hand_off(pc, pc != next) {
            c.pc = pc;
            break ST_REDISPATCH;
        }
        first = false;
        let word = core::ptr::read_unaligned(pc as *const u32);
        steps += 1;
        next = pc.wrapping_add(4);
        match exec::step(c, word, pc) {
            Flow::Next => pc = pc.wrapping_add(4),
            // a register branch may be a non-local jump (a longjmp, an unwinder's resume) into a frame below this
            // one: when another frame owns the target, through rt_dispatch, which takes it there
            Flow::Jump(t) if word & 0xFF9F_FC1F == 0xD61F_0000 && escapes(c, t, slot) => {
                c.pc = t;
                break ST_REDISPATCH;
            }
            Flow::Jump(t) => pc = t,
            Flow::Call(t) => {
                add_site(slot, pc);
                c.pc = t;
                match call(c) {
                    0 => pc = pc.wrapping_add(4),
                    1 => break ST_REDISPATCH, // c.pc holds the jump's target
                    _ => break crate::boundary::exit_jump(c),
                }
            }
            Flow::Ret => break ST_RETURN,
            Flow::Svc(imm) => {
                let st = crate::svc::nxrt_svc_impl(c, imm, pc);
                if st != ST_RETURN {
                    break st;
                }
                pc = pc.wrapping_add(4);
            }
            Flow::Brk(imm) => {
                STEPS.fetch_add(steps, Ordering::Relaxed);
                crate::rt::rt_brk(c, imm, pc)
            }
            Flow::Udf(msg) => {
                c.pc = pc;
                crate::rt::guest_fatal(
                    c,
                    &format!("interp: {} at {} is not an instruction the translator handles ({msg})", hexw(word), crate::dispatch::guest_name(pc)),
                )
            }
        }
    };
    STEPS.fetch_add(steps, Ordering::Relaxed);
    debug_assert!(status == ST_RETURN || status == ST_REDISPATCH || status == ST_JUMP);
    status
}

/// Whether a register branch to `t` from the frame of `slot` goes to another frame (`sjlj::jump_target`).
unsafe fn escapes(c: &mut Cpu, t: u64, slot: usize) -> bool {
    let pc = c.pc;
    c.pc = t;
    let to = crate::sjlj::jump_target(c);
    c.pc = pc;
    to.is_some_and(|s| s as usize != slot)
}

fn hexw(w: u32) -> String {
    format!("{w:08x}")
}

/// A guest call from the interpreter (BL / BLR; `c.pc` the target, x30 the return address) under a barrier:
/// 0 returned, 1 a jump escaped past the interpreter, 2 the thread exits.
unsafe fn call(c: &mut Cpu) -> u64 {
    crate::asm::nxrt_call_guest(c)
}

extern "C" {
    /// The `GuestFn` dispatch jumps to for an interpreted address: `rt_sj_owner(c, nxrt_interp_entry, c->pc, end)`.
    pub fn nxrt_interp_owner(c: *mut Cpu);
    /// The body: calls `nxrt_interp_invoke` and acts on its status as `nxrt_host_entry` does.
    pub fn nxrt_interp_entry(c: *mut Cpu);
}

#[cfg(windows)]
core::arch::global_asm!(
    ".text",
    ".globl nxrt_interp_owner",
    ".p2align 4",
    ".seh_proc nxrt_interp_owner",
    "nxrt_interp_owner:",
    "sub rsp, 40",
    ".seh_stackalloc 40",
    ".seh_endprologue",
    "mov qword ptr [rsp + 32], rcx",
    "call nxrt_interp_span_end",
    "mov rcx, qword ptr [rsp + 32]",
    "mov r9, rax",
    "mov r8, qword ptr [rcx + 264]",
    "lea rdx, [rip + nxrt_interp_entry]",
    "add rsp, 40",
    "jmp rt_sj_owner",
    ".seh_endproc",
    "",
    ".globl nxrt_interp_entry",
    ".p2align 4",
    ".seh_proc nxrt_interp_entry",
    "nxrt_interp_entry:",
    "push rcx",
    ".seh_pushreg rcx",
    "sub rsp, 32",
    ".seh_stackalloc 32",
    ".seh_endprologue",
    "call nxrt_interp_invoke",
    "add rsp, 32",
    "pop rcx",
    "test eax, eax",
    "jnz 2f",
    "ret",
    "2:",
    "cmp eax, 1",
    "je rt_dispatch",
    "jmp nxrt_jump_pending",
    ".seh_endproc",
);

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
core::arch::global_asm!(
    ".text",
    ".globl nxrt_interp_owner",
    ".type nxrt_interp_owner,@function",
    ".p2align 4",
    "nxrt_interp_owner:",
    ".cfi_startproc",
    "push rdi",
    ".cfi_adjust_cfa_offset 8",
    "call nxrt_interp_span_end",
    "pop rdi",
    ".cfi_adjust_cfa_offset -8",
    "mov rcx, rax",
    "mov rdx, qword ptr [rdi + 264]",
    "lea rsi, [rip + nxrt_interp_entry]",
    "jmp rt_sj_owner",
    ".cfi_endproc",
    ".size nxrt_interp_owner, .-nxrt_interp_owner",
    "",
    ".globl nxrt_interp_entry",
    ".type nxrt_interp_entry,@function",
    ".p2align 4",
    "nxrt_interp_entry:",
    ".cfi_startproc",
    "push rdi",
    ".cfi_adjust_cfa_offset 8",
    "call nxrt_interp_invoke",
    "pop rdi",
    ".cfi_adjust_cfa_offset -8",
    "test eax, eax",
    "jnz 2f",
    "ret",
    "2:",
    "cmp eax, 1",
    "je rt_dispatch",
    "jmp nxrt_jump_pending",
    ".cfi_endproc",
    ".size nxrt_interp_entry, .-nxrt_interp_entry",
);

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
core::arch::global_asm!(
    ".text",
    ".globl nxrt_interp_owner",
    ".type nxrt_interp_owner,%function",
    ".p2align 4",
    "nxrt_interp_owner:",
    ".cfi_startproc",
    "stp x0, x30, [sp, #-16]!",
    ".cfi_def_cfa_offset 16",
    ".cfi_offset w30, -8",
    "bl nxrt_interp_span_end",
    "mov x3, x0",
    "ldp x0, x30, [sp], #16",
    ".cfi_def_cfa_offset 0",
    ".cfi_restore w30",
    "ldr x2, [x0, #264]",
    "adrp x1, nxrt_interp_entry",
    "add x1, x1, :lo12:nxrt_interp_entry",
    "b rt_sj_owner",
    ".cfi_endproc",
    ".size nxrt_interp_owner, .-nxrt_interp_owner",
    "",
    ".globl nxrt_interp_entry",
    ".type nxrt_interp_entry,%function",
    ".p2align 4",
    "nxrt_interp_entry:",
    ".cfi_startproc",
    "stp x0, x30, [sp, #-16]!",
    ".cfi_def_cfa_offset 16",
    ".cfi_offset w30, -8",
    "bl nxrt_interp_invoke",
    "mov w9, w0",
    "ldp x0, x30, [sp], #16",
    ".cfi_def_cfa_offset 0",
    ".cfi_restore w30",
    "cbnz w9, 2f",
    "ret",
    "2:",
    "cmp w9, #1",
    "b.eq rt_dispatch",
    "b nxrt_jump_pending",
    ".cfi_endproc",
    ".size nxrt_interp_entry, .-nxrt_interp_entry",
);

#[cfg(test)]
mod tests;

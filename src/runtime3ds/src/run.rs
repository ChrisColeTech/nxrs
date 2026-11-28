//! the run loop: budget slices over the Entry tables, the four exits, the interpreter for
//! uncovered code and the recompiler's punts, and the settle at every boundary into the host
//! (the trail's mutex state, the ticks). docs/interface.md's minimal-host shape, with the parts
//! a working host adds.

use crate::context::{self, Code, Context};
use crate::hostfast;
use crate::interp;
use crate::svc;
use crate::thread::ThreadState;
use crate::Process3ds;
use std::sync::atomic::{AtomicU64, Ordering};

/// send-back evidence probes (set RUNTIME3DS_PROBE=1); bounded, the boot log only
static PROBE_BUDGET: AtomicU64 = AtomicU64::new(0);
static PROBE_RETURN: AtomicU64 = AtomicU64::new(0);
static PROBE_FAULT: AtomicU64 = AtomicU64::new(0);
static PROBE_ENTRY: AtomicU64 = AtomicU64::new(0);

fn probe() -> bool {
    std::env::var_os("RUNTIME3DS_PROBE").is_some()
}

fn sp_words(process: &Process3ds, sp: u32) -> String {
    let mut text = String::new();
    for i in 0..8u32 {
        text.push_str(&format!("{:#x} ", process.memory.read32(sp.wrapping_add(i * 4))));
    }
    text
}

const ROUTINE_START: u32 = 0x0033_683C;
const ROUTINE_END: u32 = 0x0033_68A0;

fn in_routine(address: u32) -> bool {
    (ROUTINE_START..ROUTINE_END).contains(&(address & !1))
}

/// the Host struct the generated code reads; the callbacks reach the thread through ctx.user
pub(crate) static HOST: context::Host = context::Host {
    read8: cb_read8,
    read16: cb_read16,
    read32: cb_read32,
    write8: cb_write8,
    write16: cb_write16,
    write32: cb_write32,
    interpret: cb_interpret,
    lookup: Some(cb_lookup),
    svc: svc::svc_callback,
};

/// the host callbacks a hand-driven context needs (the tests drive single threads directly)
pub fn host() -> &'static context::Host {
    &HOST
}

unsafe extern "C" fn cb_read8(ctx: *mut Context, address: u32) -> u8 {
    unsafe { ThreadState::of(ctx).process().memory.read8(address) }
}
unsafe extern "C" fn cb_read16(ctx: *mut Context, address: u32) -> u16 {
    unsafe { ThreadState::of(ctx).process().memory.read16(address) }
}
unsafe extern "C" fn cb_read32(ctx: *mut Context, address: u32) -> u32 {
    unsafe { ThreadState::of(ctx).process().memory.read32(address) }
}
unsafe extern "C" fn cb_write8(ctx: *mut Context, address: u32, value: u8) {
    unsafe { ThreadState::of(ctx).process().memory.write8(address, value) }
}
unsafe extern "C" fn cb_write16(ctx: *mut Context, address: u32, value: u16) {
    unsafe { ThreadState::of(ctx).process().memory.write16(address, value) }
}
unsafe extern "C" fn cb_write32(ctx: *mut Context, address: u32, value: u32) {
    unsafe { ThreadState::of(ctx).process().memory.write32(address, value) }
}

/// the recompiler's punt: one instruction through the interpreter; a step that moved control
/// hands back to the host loop with EXIT_UNWIND, a straight one goes on
unsafe extern "C" fn cb_interpret(ctx: *mut Context, address: u32, _opcode: u32) {
    let plain = address & !(((*ctx).thumb as u32) & 1);
    let process_ptr = unsafe { ThreadState::of(ctx).process };
    // safe: the process outlives the boot
    let process: &Process3ds = unsafe { &*process_ptr };
    let thumb = plain != address;
    let mut log = process.miss_log();
    let mut sink = |c: &mut Context, number: u32| {
        let state = unsafe { ThreadState::of(c) };
        process.svc_exec(state, number);
    };
    // safe: the interpreter runs on the thread's own host thread, the state lives for the boot
    let ctx_ref = unsafe { &mut *ctx };
    ctx_ref.r[15] = plain;
    let step = interp::step(ctx_ref, &process.memory, plain, &mut log, &mut sink);
    process.stats.interpreted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match step {
        interp::Step::Next => {
            ctx_ref.r[15] = plain + if thumb { 2 } else { 4 };
        }
        interp::Step::Svc(_) => {
            // the sink answered it or asked the host loop to take over (exit is set)
        }
        _ => {
            ctx_ref.exit = context::EXIT_UNWIND;
        }
    }
}

/// the code lookup the generated code asks for on indirect jumps and returns
unsafe extern "C" fn cb_lookup(ctx: *mut Context, address: u32) -> Option<Code> {
    unsafe { ThreadState::of(ctx).process().library.lock().unwrap().lookup(address) }
}

/// what one slice came to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceOutcome {
    /// the thread goes on
    Continue,
    /// the thread parked in an svc (blocked or exited); the host loop stops driving it
    Parked,
    /// the boot is over
    Stopped,
}

/// runs one budget slice for the thread: code where the tables have it, the interpreter where
/// they don't; settles the trail on the way out
pub fn run_slice(process: &Process3ds, state: &mut ThreadState, slice: i32) -> SliceOutcome {
    if state.ctx.tick_at == 0 {
        hostfast::open_tick_window(&mut state.ctx, process.tick_claim(slice as u64), slice);
    }
    state.ctx.budget = slice;
    loop {
        if process.stopped() {
            return SliceOutcome::Stopped;
        }
        if state.ctx.budget <= 0 {
            break;
        }
        let address = state.ctx.r[15] | state.ctx.thumb as u32;
        let jumped_from = address;
        let code = process.library.lock().unwrap().lookup(address);
        let before = state.ctx.budget;
        if let Some(code) = code {
            state.ctx.exit = context::EXIT_NONE;
            state.ctx.depth = 0;
            state.ctx.stack_limit = context::stack_limit();
            // safe: the code lives as long as the loaded library, the state as long as the boot
            let _returned = unsafe { code(&mut state.ctx) };
            let spent = (before - state.ctx.budget).max(0) as u64;
            process.stats.recompiled.fetch_add(spent, std::sync::atomic::Ordering::Relaxed);
            if probe() {
                let pc = state.ctx.r[15] & !1;
                if state.ctx.exit == context::EXIT_BUDGET
                    && pc == 0x0033_6854
                    && PROBE_BUDGET.fetch_add(1, Ordering::Relaxed) < 3
                {
                    process.say(format!(
                        "probe budget-cut: from {jumped_from:#x} to pc {:#x} exit {} lr {:#x} sp {:#x} r0 {:#x} r1 {:#x} sp: {}",
                        state.ctx.r[15], state.ctx.exit, state.ctx.r[14], state.ctx.r[13], state.ctx.r[0], state.ctx.r[1],
                        sp_words(process, state.ctx.r[13])
                    ));
                }
                if in_routine(jumped_from) && !in_routine(pc) && PROBE_RETURN.fetch_add(1, Ordering::Relaxed) < 1 {
                    process.say(format!(
                        "probe return: from {jumped_from:#x} to pc {:#x} exit {} lr {:#x} sp {:#x} r0 {:#x} r1 {:#x} sp: {}",
                        state.ctx.r[15], state.ctx.exit, state.ctx.r[14], state.ctx.r[13], state.ctx.r[0], state.ctx.r[1],
                        sp_words(process, state.ctx.r[13])
                    ));
                }
                let stats = process.memory.stats();
                if stats.faults > 0 && PROBE_FAULT.fetch_add(1, Ordering::Relaxed) < 1 {
                    process.say(format!(
                        "probe first fault: {} at {:#x} after {jumped_from:#x} pc {:#x} r0 {:#x} r1 {:#x}",
                        stats.faults, stats.last_fault, state.ctx.r[15], state.ctx.r[0], state.ctx.r[1]
                    ));
                }
            }
            match state.ctx.exit {
                context::EXIT_NONE | context::EXIT_UNWIND => {}
                context::EXIT_SVC => {
                    // the svc callback asked the host loop: answer it here
                    let number = state.ctx.svc;
                    process.svc_exec(state, number);
                }
                context::EXIT_BUDGET if spent == 0 => {
                    // the block did not fit the budget: one instruction through the interpreter
                    interpreter_step(process, state);
                    state.ctx.budget -= 1;
                }
                _ => {}
            }
        } else {
            // uncovered code: the interpreter runs until it reaches an entry or the budget ends
            let first = process.note_interp(state.ctx.r[15]);
            let total = process.interp_total();
            if first || total <= 12 || total % 1000 == 0 {
                process.say(format!(
                    "interpreter entry {total} at {:#x}: r0 {:#x} r1 {:#x} r2 {:#x} r3 {:#x} lr {:#x}",
                    state.ctx.r[15], state.ctx.r[0], state.ctx.r[1], state.ctx.r[2], state.ctx.r[3], state.ctx.r[14]
                ));
            }
            if probe() && PROBE_ENTRY.fetch_add(1, Ordering::Relaxed) < 3 {
                process.say(format!(
                    "probe interp resume: pc {:#x} lr {:#x} sp {:#x} r0 {:#x} r1 {:#x} c {} sp: {}",
                    state.ctx.r[15], state.ctx.r[14], state.ctx.r[13], state.ctx.r[0], state.ctx.r[1], state.ctx.c,
                    sp_words(process, state.ctx.r[13])
                ));
            }
            let interp_from = state.ctx.r[15];
            let mut miss_log = process.miss_log();
            let lookup = |address: u32| process.library.lock().unwrap().lookup(address);
            let mut sink = |c: &mut Context, number: u32| {
                let state = unsafe { ThreadState::of(c) };
                process.svc_exec(state, number);
            };
            interp::run_block(&mut state.ctx, &process.memory, &lookup, &mut miss_log, &mut sink, 4096);
            let spent = (before - state.ctx.budget).max(0) as u64;
            process.stats.interpreted.fetch_add(spent, std::sync::atomic::Ordering::Relaxed);
            if probe() {
                let pc = state.ctx.r[15] & !1;
                if in_routine(interp_from) && !in_routine(pc) && PROBE_RETURN.fetch_add(1, Ordering::Relaxed) < 1 {
                    process.say(format!(
                        "probe interp return: from {interp_from:#x} to pc {:#x} lr {:#x} sp {:#x} r0 {:#x} r1 {:#x} c {} sp: {}",
                        state.ctx.r[15], state.ctx.r[14], state.ctx.r[13], state.ctx.r[0], state.ctx.r[1], state.ctx.c,
                        sp_words(process, state.ctx.r[13])
                    ));
                }
                let stats = process.memory.stats();
                if stats.faults > 0 && PROBE_FAULT.fetch_add(1, Ordering::Relaxed) < 1 {
                    process.say(format!(
                        "probe first fault (interp branch): {} at {:#x} from {interp_from:#x} pc {:#x} r0 {:#x} r1 {:#x}",
                        stats.faults, stats.last_fault, state.ctx.r[15], state.ctx.r[0], state.ctx.r[1]
                    ));
                }
            }
        }
        // the settle: the loan back to the kernel, the trail counted, the tick site forgotten
        let settled = hostfast::settle(&mut state.ctx);
        process.apply_settled(state, settled);
        // a parking svc (sleep, a wait, an arbiter hold, an exit) stops this host loop's drive
        if process.waiting_of(state.thread).is_some() {
            return SliceOutcome::Parked;
        }
    }
    if process.stopped() {
        return SliceOutcome::Stopped;
    }
    SliceOutcome::Continue
}

/// one interpreter instruction, for a budget that cannot fit a block
fn interpreter_step(process: &Process3ds, state: &mut ThreadState) {
    let thumb = state.ctx.thumb != 0;
    let plain = state.ctx.r[15] & if thumb { !1 } else { !3 };
    let mut log = process.miss_log();
    let mut sink = |c: &mut Context, number: u32| {
        let state = unsafe { ThreadState::of(c) };
        process.svc_exec(state, number);
    };
    state.ctx.r[15] = plain;
    match interp::step(&mut state.ctx, &process.memory, plain, &mut log, &mut sink) {
        interp::Step::Next => {
            state.ctx.r[15] = plain + if thumb { 2 } else { 4 };
        }
        interp::Step::Svc(_) => {}
        _ => state.ctx.exit = context::EXIT_UNWIND,
    }
    process.stats.interpreted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

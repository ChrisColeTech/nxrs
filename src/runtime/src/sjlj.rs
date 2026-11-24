//! Guest setjmp/longjmp, and any other non-local jump into the middle of a guest function, across lifted code.
//!
//! The guest's setjmp and longjmp run as lifted code and save and restore the guest registers themselves. What
//! lifting breaks is longjmp's last instruction, `br x30` to the setjmp call's return address: the middle of a C
//! function, whose frame was left long ago. So the generator builds every function that may be entered that way (its
//! "owner": today every caller of setjmp) with all its labels and enters its body through `rt_sj_owner` (`asm.rs`),
//! which keeps a host jump point in a frame that lives as long as the owner does. A dispatch that finds no function at
//! an address strictly inside a live owner is such a jump: the guest registers are restored already, so the host jumps
//! back to the owner's frame and the body continues at `c->pc`. The same mechanism serves an exception unwinder's
//! resume at a landing pad, once the generator makes the functions holding landing pads owners (README, "Requests").
//!
//! Host -> guest calls run under barrier slots in the same chain (`nxrt_call_guest`). A jump whose owner lies below a
//! barrier goes to the barrier first, so the Rust host function between them returns normally before the jump goes
//! on: no jump ever skips a live Rust frame.

use crate::asm::{SjSlot, BARRIER_ESCAPE, BARRIER_EXIT};
use crate::cpu::Cpu;
use std::sync::atomic::{AtomicBool, Ordering};

static LOGGED: AtomicBool = AtomicBool::new(false);

/// The slot a guest jump to `c->pc` goes to: the newest owner holding `c->pc`, or the newest barrier above such an
/// owner (its status set to `BARRIER_ESCAPE`). None when no live owner holds it. `c->sj` is not changed.
///
/// # Safety
/// `c` is the calling thread's guest state with a live chain.
pub unsafe fn jump_target(c: *const Cpu) -> Option<*mut SjSlot> {
    let pc = (*c).pc;
    // an interpreter frame owns exact resume points, not a range (`interp::owns`)
    let inside = |s: &SjSlot| {
        !s.is_barrier()
            && if crate::interp::is_slot(s) {
                crate::interp::owns(s, &*c)
            } else {
                pc > s.lo && pc < s.hi
            }
    };
    let mut s = (*c).sj as *mut SjSlot;
    while !s.is_null() {
        if (*s).is_barrier() {
            let mut t = (*s).up;
            while !t.is_null() {
                if inside(&*t) {
                    (*s).status = BARRIER_ESCAPE;
                    return Some(s);
                }
                t = (*t).up;
            }
            return None;
        }
        if inside(&*s) {
            if !LOGGED.swap(true, Ordering::Relaxed) || crate::rt::trace("sjlj") {
                crate::rt::log(&format!(
                    "sjlj: jump to {} resumes owner {}",
                    crate::dispatch::guest_name(pc),
                    crate::dispatch::guest_name((*s).lo)
                ));
            }
            return Some(s);
        }
        s = (*s).up;
    }
    None
}

/// The barrier ExitThread unwinds to next (the thread's base is its outermost barrier), status `BARRIER_EXIT`.
///
/// # Safety
/// As `jump_target`.
pub unsafe fn exit_target(c: *const Cpu) -> Option<*mut SjSlot> {
    let mut s = (*c).sj as *mut SjSlot;
    while !s.is_null() {
        if (*s).is_barrier() {
            (*s).status = BARRIER_EXIT;
            return Some(s);
        }
        s = (*s).up;
    }
    None
}

//! the golden driver: a bare Context + VFP bank + one memory, one instruction at a time through
//! `interp::step`, stopped at the return sentinel. No code library, no compiler, seconds-fast.

use runtime3ds::context::Context;
use runtime3ds::interp::{self, NoLog};
use runtime3ds::mem::{Kind, Memory, Perm, State};

pub const BASE: u32 = 0x0010_0000;
pub const HEAP: u32 = 0x1000_0000;
pub const STACK_TOP: u32 = 0x0FF0_0000;
/// the return nothing maps, like the donor's verify lane
pub const RETURN: u32 = 0xFFFF_FFF0;
const CAP: usize = 200;

pub struct Machine {
    pub ctx: Box<Context>,
    pub vfp: Box<[u32; 32]>,
    pub fpscr: &'static mut u32,
    pub memory: &'static Memory,
}

/// one machine: the text, a heap page and a stack page mapped, r0 at the heap, sp at the top,
/// lr at the return sentinel
pub fn setup() -> Machine {
    let memory: &'static Memory = Box::leak(Box::new(Memory::new().unwrap()));
    memory.map(BASE, 0x1000, Perm::R, State::Code, Kind::Exe).unwrap();
    memory.map(HEAP, 0x1000, Perm::Rw, State::Private, Kind::Heap).unwrap();
    memory.map(STACK_TOP - 0x1000, 0x1000, Perm::Rw, State::Private, Kind::Stack).unwrap();
    let mut vfp = Box::new([0u32; 32]);
    let machine_fpscr: &'static mut u32 = Box::leak(Box::new(0u32));
    let mut ctx = Box::new(Context {
        r: [0; 16],
        n: 0,
        z: 0,
        c: 0,
        v: 0,
        q: 0,
        thumb: 0,
        ge: 0,
        exclusive: 0,
        budget: 0,
        exit: 0,
        svc: 0,
        depth: 0,
        exclusive_address: 0,
        tls: 0,
        read_pages: memory.read_table(),
        write_pages: memory.write_table(),
        vfp: std::ptr::null_mut(),
        fpscr: std::ptr::null_mut(),
        host: std::ptr::null(),
        user: std::ptr::null_mut(),
        stack_limit: 0,
        tick_end: 0,
        tick_last: 0,
        tick_floor: 0,
        tick_cost: 0,
        tick_at: 0,
        ticks: 0,
        wait_at: 0,
        release_at: 0,
        held: 0,
        holding: 0,
        held_floor: 0,
        done: 0,
        trail: [0; 64],
    });
    ctx.vfp = vfp.as_mut_ptr();
    ctx.fpscr = machine_fpscr;
    ctx.r[0] = HEAP;
    ctx.r[13] = STACK_TOP;
    ctx.r[14] = RETURN;
    Machine { ctx, vfp, fpscr: machine_fpscr, memory }
}

pub fn load(machine: &mut Machine, words: &[u32]) {
    let mut bytes = Vec::new();
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    machine.memory.load_bytes(BASE, &bytes);
    machine.ctx.r[15] = BASE;
    machine.ctx.thumb = 0;
}

pub fn load_thumb(machine: &mut Machine, halves: &[u16]) {
    let mut bytes = Vec::new();
    for half in halves {
        bytes.extend_from_slice(&half.to_le_bytes());
    }
    machine.memory.load_bytes(BASE, &bytes);
    machine.ctx.r[15] = BASE | 1;
    machine.ctx.thumb = 1;
}

/// walks one instruction at a time until the return sentinel or the cap
pub fn walk(machine: &mut Machine) {
    for _ in 0..CAP {
        let address = machine.ctx.r[15];
        if machine.ctx.thumb == 0 && address == RETURN {
            return;
        }
        let plain = address & if machine.ctx.thumb != 0 { !1 } else { !3 };
        machine.ctx.r[15] = plain;
        let mut log = NoLog;
        let mut sink = |_ctx: &mut Context, number: u32| {
            panic!("the golden set runs no svcs, got {number:#x}");
        };
        machine.ctx.fpscr = machine.fpscr;
        machine.ctx.vfp = machine.vfp.as_mut_ptr();
        match interp::step(&mut machine.ctx, machine.memory, plain, &mut log, &mut sink) {
            interp::Step::Next => {
                let size = if machine.ctx.thumb != 0 { 2 } else { 4 };
                machine.ctx.r[15] = plain + size;
            }
            interp::Step::Taken => {}
            other => panic!("the golden set walks straight lines only, got {other:?} at {plain:#x}"),
        }
    }
    panic!("the golden walk hit its cap at {:#x}", machine.ctx.r[15]);
}

pub fn run_arm(machine: &mut Machine, words: &[u32]) {
    load(machine, words);
    walk(machine);
}

pub fn run_thumb(machine: &mut Machine, halves: &[u16]) {
    load_thumb(machine, halves);
    walk(machine);
}

pub fn vfp_word(machine: &Machine, r: usize) -> u32 {
    machine.vfp[r]
}

pub fn set_vfp_word(machine: &mut Machine, r: usize, bits: u32) {
    machine.vfp[r] = bits;
}

pub fn set_fpscr(machine: &mut Machine, value: u32) {
    *machine.fpscr = value;
}

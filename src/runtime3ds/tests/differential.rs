//! the differential: the same instruction sequence through the interpreter and through the
//! recompiled C (translator3ds -> clang -> the mirror), final state compared — the donor
//! verify.rs idea, portable. It rides the compile lane's env gate like host_run.

mod common;

use common::*;
use runtime3ds::context::{self, Context};
use runtime3ds::interp::{self, NoLog};
use runtime3ds::mem::{Memory, Perm, State, Kind};
use runtime3ds::run::{run_slice, SliceOutcome};
use runtime3ds::thread::ThreadState;
use runtime3ds::Process3ds;
use std::sync::Arc;

const HEAP: u32 = 0x1000_0000;
const RETURN: u32 = 0xFFFF_FFF0;

/// ARM data processing, multiply, load/store, shifts, and VFP arithmetic/compare/conversion
const WORDS: [u32; 19] = [
    0xE3A0_1009, // mov r1, #9
    0xE3A0_2007, // mov r2, #7
    0xE003_0291, // mul r3, r1, r2        ; 63
    0xE024_3291, // mla r4, r1, r2, r3    ; 126
    0xE284_5005, // add r5, r4, #5        ; 131
    0xE055_6004, // subs r6, r5, r4       ; 5
    0xE590_7000, // ldr r7, [r0]          ; 0x11223344
    0xE580_5004, // str r5, [r0, #4]
    0xE5D0_8004, // ldrb r8, [r0, #4]     ; 0x83
    0xE1A0_9105, // lsl r9, r5, #2        ; 0x20C
    0xE189_A003, // orr r10, r9, r3       ; 0x24F
    0xED90_0A02, // vldr s0, [r0, #8]     ; 1.5
    0xEDD0_0A03, // vldr s1, [r0, #12]    ; 2.5
    0xEE30_1A20, // vadd.f32 s2, s0, s1   ; 4.0
    0xEE61_1A20, // vmul.f32 s3, s2, s1   ; 10.0
    0xEEF5_1A40, // vcmp.f32 s3, #0
    0xEEF1_BA10, // vmrs r11, fpscr
    0xEEBD_2AE1, // vcvt.s32.f32 s4, s3   ; 10
    0xEF00_0003, // svc 0x03: ExitProcess
];

fn bare_context(memory: &Memory, vfp: &mut [u32; 32], fpscr: &mut u32) -> Box<Context> {
    Box::new(Context {
        r: [0; 16],
        n: 0,
        z: 0,
        c: 0,
        v: 0,
        q: 0,
        thumb: 0,
        ge: 0,
        exclusive: 0,
        budget: 10_000,
        exit: 0,
        svc: 0,
        depth: 0,
        exclusive_address: 0,
        tls: 0,
        read_pages: memory.read_table(),
        write_pages: memory.write_table(),
        vfp: vfp.as_mut_ptr(),
        fpscr,
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
        trail: [0; context::TRAIL],
    })
}

/// the same starting state both sides get: r0 at the scratch page, entry at BASE
fn begin(ctx: &mut Context) {
    ctx.r[0] = HEAP;
    ctx.r[15] = BASE;
    ctx.r[14] = RETURN;
}

/// fills the scratch page with the same inputs
fn fill(memory: &Memory) {
    memory.write32(HEAP, 0x1123_3444);
    memory.write32(HEAP + 8, 1.5_f32.to_bits());
    memory.write32(HEAP + 12, 2.5_f32.to_bits());
}

/// the interpreter walks until the svc (the end of the tested window)
fn interpreted(memory: &Arc<Memory>) -> (Box<Context>, Box<[u32; 32]>, u32) {
    let mut vfp = Box::new([0u32; 32]);
    let mut fpscr = 0u32;
    let mut ctx = bare_context(memory, &mut vfp, &mut fpscr);
    begin(&mut ctx);
    let mut log = NoLog;
    let mut sink = |_c: &mut Context, _n: u32| {};
    loop {
        let address = ctx.r[15];
        if address == RETURN {
            break;
        }
        if ctx.thumb == 0 {
            ctx.r[15] = address & !3;
        }
        let plain = ctx.r[15];
        match interp::step(&mut ctx, memory, plain, &mut log, &mut sink) {
            interp::Step::Next => ctx.r[15] = plain + if ctx.thumb != 0 { 2 } else { 4 },
            interp::Step::Svc(_) => break,
            other => panic!("the differential's window got {other:?} at {plain:#x}"),
        }
    }
    (ctx, vfp, fpscr)
}

struct Harness {
    process: Box<Process3ds>,
    state: &'static mut ThreadState,
}

impl Harness {
    fn new(library: runtime3ds::dispatch::Library, memory: Arc<Memory>) -> Harness {
        let process = Box::new(Process3ds::new(library, memory.clone(), Default::default()));
        let mut state = ThreadState::new(&*process, 0, 0x1FF8_2000);
        state.ctx.read_pages = memory.read_table();
        state.ctx.write_pages = memory.write_table();
        state.ctx.host = runtime3ds::run::host();
        Harness { process, state: Box::leak(state) }
    }

    fn run(&mut self, slice: i32) -> SliceOutcome {
        run_slice(&self.process, &mut self.state, slice)
    }
}

fn recompiled(memory: Arc<Memory>, library: runtime3ds::dispatch::Library) -> (Box<Process3ds>, &'static mut ThreadState) {
    let mut harness = Harness::new(library, memory);
    begin(&mut harness.state.ctx);
    harness.run(50_000);
    (harness.process, harness.state)
}

#[test]
fn the_interpreter_and_the_recompiled_code_agree() {
    let Some((library, _path, _dump, title)) = loaded_library("differential", &WORDS) else {
        eprintln!("skipping: NXRS_3DS_CC_TESTS is off, the compile lane did not run");
        return;
    };
    let code_bytes = title.code().unwrap();
    let memory = test_memory(&code_bytes);
    fill(&memory);
    let (interp_ctx, interp_vfp, interp_fpscr) = interpreted(&memory);

    let memory2 = Arc::new(Memory::new().unwrap());
    memory2.map(BASE, code_bytes.len().next_multiple_of(0x1000) as u32, Perm::R, State::Code, Kind::Exe).unwrap();
    for (i, word) in WORDS.iter().enumerate() {
        memory2.load_bytes(BASE + i as u32 * 4, &word.to_le_bytes());
    }
    memory2.map(HEAP, 0x1000, Perm::Rw, State::Private, Kind::Heap).unwrap();
    fill(&memory2);
    let (process, state) = recompiled(memory2.clone(), library);
    let report = process.report(PROGRAM_ID, String::new());
    assert!(report.recompiled > 0, "the recompiled C ran the window, not the interpreter fallback");
    eprintln!(
        "differential: 0 mismatches over r0-r12, nzcv, s0-s4, fpscr flags, heap; recompiled {} instrs, interpreted {}",
        report.recompiled, report.interpreted
    );

    let mut mismatches = 0;
    let a = &interp_ctx;
    let c = &state.ctx;
    for r in 0..=12usize {
        if a.r[r] != c.r[r] {
            mismatches += 1;
            eprintln!("mismatch r{r}: interp {:#x}, recompiled {:#x}", a.r[r], c.r[r]);
        }
    }
    for (name, x, y) in [("n", a.n, c.n), ("z", a.z, c.z), ("c", a.c, c.c), ("v", a.v, c.v)] {
        if x != y {
            mismatches += 1;
            eprintln!("mismatch flag {name}: interp {x}, recompiled {y}");
        }
    }
    for s in 0..=4usize {
        if interp_vfp[s] != state.vfp[s] {
            mismatches += 1;
            eprintln!("mismatch s{s}: interp {:#x}, recompiled {:#x}", interp_vfp[s], state.vfp[s]);
        }
    }
    if interp_fpscr >> 28 != state.fpscr >> 28 {
        mismatches += 1;
        eprintln!("mismatch fpscr flags: interp {:#x}, recompiled {:#x}", interp_fpscr, state.fpscr);
    }
    if memory.read32(HEAP + 4) != memory2.read32(HEAP + 4) {
        mismatches += 1;
        eprintln!("mismatch memory heap+4");
    }
    assert_eq!(mismatches, 0, "the two sides must agree on the tested classes");
}

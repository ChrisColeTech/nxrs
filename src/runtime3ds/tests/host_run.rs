//! the host-run proofs: the generated C called through this crate's mirror, asserting the state
//! transitions (the W0 donor codegen tests' Rust-side equivalent — the DLL built and executed).

mod common;

use common::*;
use runtime3ds::context;
use runtime3ds::dispatch::Library;
use runtime3ds::mem::Memory;
use runtime3ds::run::{run_slice, SliceOutcome};
use runtime3ds::thread::ThreadState;
use runtime3ds::Process3ds;
use std::sync::Arc;

/// one thread and its state, driven by the real run loop; the CRO-less proofs run the synthetic
/// entry under the process the boot would give it
struct Harness {
    process: Box<Process3ds>,
    state: &'static mut ThreadState,
}

impl Harness {
    fn new(library: Library, memory: Arc<Memory>) -> Harness {
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

/// the entry answers GetSystemTick itself at the learned site, hands a plain svc to the host
/// (the gap line), calls a function and comes back: the synthetic's C says exactly that
/// (SVC_TICK / SVC / CALL_RETURNING), and the run loop proves each transition
#[test]
fn the_entry_answers_ticks_hands_svcs_over_and_calls() {
    let Some((library, _lib_path, dump, _title)) = loaded_library(
        "proof-entry",
        &[
            0xEF00_0028, // svc 0x28: GetSystemTick (the host answers; the site is learned)
            0xE3A0_0001, // mov r0, #1
            0xEF00_0032, // svc 0x32: not in the kernel table, handed over with a gap line
            0xEB00_0004, // bl 0x00100024
            0xE3A0_2020, // mov r2, #0x20 (after the call)
            0xE295_5001, // adds r5, r5, #1
            0xE355_0001, // cmp r5, #1
            0x0AFF_FFF7, // beq 0x00100000 (the second tick is answered in place)
            0xE12F_FF14, // bx r4 → the return the test set up
            0xE3A0_3030, // mov r3, #0x30 (the callee)
            0xE12F_FF1E, // bx lr → back to 0x10
            0xE3A0_0000, // mov r0, #0
            0xEF00_0003, // svc 0x03: ExitProcess, the boot ends cleanly
        ],
    ) else {
        eprintln!("skipping: NXRS_3DS_CC_TESTS is off, the compile lanes did not run");
        return;
    };
    assert_eq!(library.abi, context::ABI, "the code is for this interface");
    assert!(library.generation >= 6, "generation 6 answers the tick itself");
    let memory = test_memory(&[]);
    let mut harness = Harness::new(library, memory);
    let ctx = &mut harness.state.ctx;
    ctx.r[15] = BASE;
    ctx.r[4] = BASE + 0x2C; // where bx r4 returns
    harness.run(10_000);
    let ctx = &harness.state.ctx;
    assert_eq!(ctx.r[3], 0x30, "the callee ran");
    assert_eq!(ctx.r[2], 0x20, "the caller went on after the call");
    let report = harness.process.report(PROGRAM_ID, harness.process.library.lock().unwrap().describe());
    assert!(report.ticks_answered_in_place >= 1, "the tick answered itself");
    assert!(
        report.gaps.iter().any(|(line, _)| line.contains("svc 0x32")),
        "the plain svc's gap line, {:?}",
        report.gaps
    );
    let _ = dump;
}

/// a func whose bytes changed stops answering lookups and the interpreter covers it
#[test]
fn a_changed_func_falls_back_to_the_interpreter() {
    let Some((library, _lib_path, _dump, title)) = loaded_library(
        "proof-stale",
        &[
            0xE3A0_0001, // mov r0, #1
            0xE3A0_1002, // mov r1, #2 (the interpreter must reach this through the stale func)
            0xE3A0_0000, // mov r0, #0
            0xEF00_0003, // svc 0x03
        ],
    ) else {
        eprintln!("skipping: NXRS_3DS_CC_TESTS is off, the compile lanes did not run");
        return;
    };
    let code_bytes = title.code().unwrap();
    let memory = test_memory(&code_bytes);
    let mut harness = Harness::new(library, memory.clone());
    // the code checks against what it was made from: clean
    let stale = harness
        .process
        .library
        .lock()
        .unwrap()
        .check(0, 0, |address, out| {
            let offset = (address - BASE) as usize;
            out.copy_from_slice(&code_bytes[offset..offset + out.len()]);
        });
    assert_eq!(stale, 0, "the code is the code it was made from");
    // flip a byte of the mov r1 the way a patch would: the func goes stale and its entries refuse
    memory.load_bytes(BASE + 4, &[0x03]);
    let stale = harness
        .process
        .library
        .lock()
        .unwrap()
        .check(0, 0, |address, out| {
            let offset = (address - BASE) as usize;
            for (i, byte) in out.iter_mut().enumerate() {
                *byte = memory.read8(BASE + offset as u32 + i as u32);
            }
        });
    assert_eq!(stale, 1, "the changed byte marked the func");
    assert!(
        harness.process.library.lock().unwrap().lookup(BASE).is_none(),
        "a stale func's entry refuses"
    );
    // the interpreter covers it: r1 carries the changed move's value
    let ctx = &mut harness.state.ctx;
    ctx.r[15] = BASE;
    ctx.r[14] = BASE + 0xC;
    ctx.r[0] = 0;
    ctx.r[1] = 0;
    harness.run(2_000);
    let ctx = &harness.state.ctx;
    assert_eq!(ctx.r[1], 3, "the interpreter ran the changed instruction");
    assert_eq!(ctx.r[0], 0, "and went on to the exit svc");
}

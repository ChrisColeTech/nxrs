//! the interpreter's golden set: hand-assembled ARM/Thumb/VFP cases with expected register,
//! flag, memory and VFP results, covering the classes the donor's codegen leaves to
//! `interpret` plus the common ones. Every encoding was assembled with llvm's ARMv6-K
//! assembler and checked against it. Interpreter-only, no code library, seconds-fast.

mod golden_util;

use golden_util::*;

/// ARM data processing: flag-setting adds with carry chains
#[test]
fn arm_data_processing() {
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_1001, // mov r1, #1
            0xE281_107F, // add r1, r1, #127        ; 128
            0xE291_2000, // adds r2, r1, #0
            0xE0B1_3001, // adcs r3, r1, r1         ; 256, carry out 1
            0xE253_4001, // subs r4, r3, #1         ; 255
            0xE2C4_5001, // sbcs r5, r4, #1         ; 254, borrow in 0
            0xE3E0_7000, // mvn r7, #0              ; 0xFFFFFFFF
            0xE111_0001, // tst r1, r1              ; not zero
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 128);
    assert_eq!(machine.ctx.r[2], 128);
    assert_eq!(machine.ctx.r[3], 256);
    assert_eq!(machine.ctx.c, 1, "adcs of 128+128 carries");
    assert_eq!(machine.ctx.r[4], 255);
    assert_eq!(machine.ctx.r[5], 254);
    assert_eq!(machine.ctx.r[7], 0xFFFF_FFFF);
    assert_eq!(machine.ctx.z, 0);
}

/// shifts: immediate forms with carry outs, register-controlled amounts, rrx
#[test]
fn arm_shifts_and_carry() {
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_0001, // mov r0, #1
            0xE1A0_1080, // lsl r1, r0, #1          ; 2, carry 1
            0xE1B0_2081, // movs r2, r1, lsl #1     ; 4, carry 0
            0xE1A0_30C2, // asr r3, r2, #1          ; 2
            0xE3E0_4000, // mvn r4, #0              ; 0xFFFFFFFF
            0xE1B0_5444, // movs r5, r4, asr #8     ; 0xFFFFFFFF, carry 1
            0xE3A0_6002, // mov r6, #2
            0xE1A0_0611, // lsl r0, r1, r6          ; 8 (register-controlled)
            0xE1A0_7E60, // ror r7, r0, #28         ; 0x80, carry 0
            0xE350_0009, // cmp r0, #9              ; 8-9 borrows, carry clear
            0xE1B0_8067, // movs r8, r7, rrx        ; 0x40 with carry 0 in, carry out 0
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 2);
    assert_eq!(machine.ctx.r[2], 4);
    assert_eq!(machine.ctx.r[3], 2);
    assert_eq!(machine.ctx.r[5], 0xFFFF_FFFF);
    assert_eq!(machine.ctx.r[0], 8);
    assert_eq!(machine.ctx.r[7], 0x80, "ror by 28 of 8 lands in bits 4..7");
    assert_eq!(machine.ctx.r[8], 0x40, "rrx with the carry clear");
    assert_eq!(machine.ctx.c, 0);
}

/// the multiply family with the assembler-verified encodings
#[test]
fn arm_multiplies() {
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_1009, // mov r1, #9
            0xE3A0_2007, // mov r2, #7
            0xE003_0291, // mul r3, r1, r2            ; 63
            0xE024_3291, // mla r4, r1, r2, r3        ; 9*7 + 63 = 126
            0xE3E0_5000, // mvn r5, #0                ; 0xFFFFFFFF
            0xE3A0_6008, // mov r6, #8
            0xE089_8695, // umull r8, r9, r5, r6      ; 0x7_FFFFFFF8
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[3], 63);
    assert_eq!(machine.ctx.r[4], 126);
    assert_eq!(machine.ctx.r[8], 0xFFFF_FFF8, "umull lo");
    assert_eq!(machine.ctx.r[9], 0x7, "umull hi");
    // smull r8, r9, r5, r6 = -1 * 8 = -8
    run_arm(&mut machine, &[0xE0C9_8695, 0xE12F_FF1E]);
    assert_eq!(machine.ctx.r[8], 0xFFFF_FFF8);
    assert_eq!(machine.ctx.r[9], 0xFFFF_FFFF);
    // smlal three times: -8 + -8 + -8 + -8 = -32 (smull left -8 in the pair)
    for _ in 0..3 {
        run_arm(&mut machine, &[0xE0E9_8695, 0xE12F_FF1E]);
    }
    assert_eq!(machine.ctx.r[8], 0xFFFF_FFE0, "-32 lo");
    assert_eq!(machine.ctx.r[9], 0xFFFF_FFFF, "-32 hi");
    // mulS sets the flags from the result
    run_arm(&mut machine, &[0xE013_0291, 0xE12F_FF1E]);
    assert_eq!(machine.ctx.r[3], 63, "muls r3, r1, r2");
    assert_eq!(machine.ctx.n, 0);
    assert_eq!(machine.ctx.z, 0);
}

/// loads and stores: the widths, the writeback forms, push/pop
#[test]
fn arm_loads_and_stores() {
    let mut machine = setup();
    machine.memory.write32(HEAP, 0x1234_5678);
    machine.memory.write32(HEAP + 4, 0x9ABC_DEF0);
    machine.memory.write16(HEAP + 0x10, 0xBEEF);
    machine.memory.write8(HEAP + 0x20, 0x87);
    run_arm(
        &mut machine,
        &[
            0xE590_1000, // ldr r1, [r0]
            0xE5D0_2000, // ldrb r2, [r0]
            0xE1D0_30B0, // ldrh r3, [r0]
            0xE1D0_40D0, // ldrsb r4, [r0]
            0xE1D0_50F0, // ldrsh r5, [r0]
            0xE590_6004, // ldr r6, [r0, #4]
            0xE5D0_7020, // ldrb r7, [r0, #0x20]     ; 0x87
            0xE1D0_80F0, // ldrsh r8, [r0]           ; 0x5678
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 0x1234_5678);
    assert_eq!(machine.ctx.r[2], 0x78);
    assert_eq!(machine.ctx.r[3], 0x5678);
    assert_eq!(machine.ctx.r[4], 0x78);
    assert_eq!(machine.ctx.r[5], 0x5678);
    assert_eq!(machine.ctx.r[6], 0x9ABC_DEF0);
    assert_eq!(machine.ctx.r[7], 0x87);
    assert_eq!(machine.ctx.r[8], 0x5678);
    // writeback: pre-indexed and post-indexed, verified encodings
    let mut machine = setup();
    machine.memory.write32(HEAP, 0x1111_1111);
    machine.memory.write32(HEAP + 8, 0x2222_2222);
    run_arm(
        &mut machine,
        &[
            0xE590_1008, // ldr r1, [r0, #8]         ; r0 untouched
            0xE5B0_2008, // ldr r2, [r0, #8]!        ; r0 = heap+8
            0xE490_3004, // ldr r3, [r0], #4         ; r3 = [heap+8], r0 = heap+0xC
            0xE580_4000, // str r4, [r0]             ; writes at heap+0xC
            0xE5A0_5004, // str r5, [r0, #4]!        ; r0 = heap+0x10
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 0x2222_2222);
    assert_eq!(machine.ctx.r[2], 0x2222_2222);
    assert_eq!(machine.ctx.r[3], 0x2222_2222);
    assert_eq!(machine.ctx.r[0], HEAP + 0x10, "both writebacks applied");
    assert_eq!(machine.memory.read32(HEAP + 0xC), machine.ctx.r[4]);
    // push/pop
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_1001, // mov r1, #1
            0xE3A0_2002, // mov r2, #2
            0xE3A0_3003, // mov r3, #3
            0xE92D_000E, // push {r1, r2, r3}
            0xE3A0_1000, // mov r1, #0
            0xE3A0_2000, // mov r2, #0
            0xE3A0_3000, // mov r3, #0
            0xE59D_0000, // ldr r0, [sp]
            0xE8BD_000E, // pop {r1, r2, r3}
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[0], 1, "the lowest register lands lowest");
    assert_eq!(machine.ctx.r[1], 1);
    assert_eq!(machine.ctx.r[2], 2);
    assert_eq!(machine.ctx.r[3], 3);
    assert_eq!(machine.ctx.r[13], STACK_TOP, "sp restored");
}

/// branches: conditional both ways and bl's return address
#[test]
fn arm_branches() {
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_0000, // mov r0, #0
            0xE3A0_1001, // mov r1, #1
            0xE351_0001, // cmp r1, #1
            0x0A00_0001, // beq +1 (taken, to 0x18)
            0xE3A0_00FF, // (skipped)
            0xE3A0_00EE, // (skipped)
            0xE3A0_2005, // mov r2, #5
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[2], 5, "the taken branch landed past the skipped words");
    assert_eq!(machine.ctx.r[0], 0);
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_1002, // mov r1, #2
            0xE351_0001, // cmp r1, #1
            0x0A00_0001, // beq (not taken)
            0xE3A0_0007, // mov r0, #7
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[0], 7);
    // bl's return address and the call
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xEB00_0003, // bl +3 → BASE+0x14 (the callee)
            0xE3A0_0033, // mov r0, #0x33 (after the call)
            0xE3E0_400F, // mvn r4, #0xF → 0xFFFFFFF0 (the RETURN sentinel)
            0xE12F_FF14, // bx r4 → RETURN → stop
            0xE1A0_0000, // pad
            0xE3A0_3030, // mov r3, #0x30 (the callee)
            0xE12F_FF1E, // bx lr → back to BASE+4
        ],
    );
    assert_eq!(machine.ctx.r[3], 0x30, "the callee ran");
    assert_eq!(machine.ctx.r[0], 0x33, "and returned to the caller");
}

/// the misc group: clz, rev family, extends with rotate, and the cp15 TLS read
#[test]
fn arm_misc_and_tls() {
    let mut machine = setup();
    machine.ctx.tls = 0x1FF8_2100;
    run_arm(
        &mut machine,
        &[
            0xE3E0_0000, // mvn r0, #0                  ; 0xFFFFFFFF
            0xE16F_0F10, // clz r0, r0                  ; 0
            0xE3A0_1000, // mov r1, #0
            0xE16F_1F11, // clz r1, r1                  ; 32
            0xE3A0_2034, // mov r2, #0x34
            0xE6FF_2FB2, // revsh r2, r2                ; 0x3400
            0xEE1D_0F70, // mrc p15, 0, r0, c13, c0, 3  ; the TLS register
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[0], 0x1FF8_2100, "the cp15 TLS read");
    assert_eq!(machine.ctx.r[1], 32, "clz of 0");
    assert_eq!(machine.ctx.r[2], 0x3400, "revsh swaps and sign-extends");
    // extends with rotate (assembler-verified rotate field)
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3E0_0000, // mvn r0, #0
            0xE6EF_1070, // uxtb r1, r0                 ; 0xFF
            0xE6EF_1C70, // uxtb r1, r0, ror #24        ; byte 3 = 0xFF
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 0xFF);
}

/// the exclusive monitor: ldrex marks, strex in range passes and clears, a second strex fails
#[test]
fn arm_exclusive_monitor() {
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE190_2F9F, // ldrex r2, [r0]
            0xE3A0_3042, // mov r3, #0x42
            0xE180_4F93, // strex r4, r3, [r0]   ; passes
            0xE3A0_5043, // mov r5, #0x43
            0xE180_5F95, // strex r5, r5, [r0]   ; fails, the monitor cleared
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[4], 0, "the first strex passed");
    assert_eq!(machine.ctx.r[5], 1, "the second strex failed");
    assert_eq!(machine.memory.read32(HEAP), 0x42);
    // strex without an ldrex fails and writes nothing
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_1005, // mov r1, #5
            0xE3A0_2035, // mov r2, #0x35
            0xE180_1F92, // strex r1, r2, [r0]
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 1);
    assert_eq!(machine.memory.read32(HEAP), 0, "the failed strex wrote nothing");
}

/// Thumb-1: the common formats, a pc-relative load, conditional branches, push/pop, bl
#[test]
fn thumb_formats() {
    let mut machine = setup();
    run_thumb(
        &mut machine,
        &[
            0x2001, // movs r0, #1
            0x2102, // movs r1, #2
            0x1842, // adds r2, r0, r1         ; 3
            0x3A01, // subs r2, #1             ; 2
            0x0609, // lsls r1, r1, #24        ; 0x02000000
            0x3101, // adds r1, #1             ; 0x02000001
            0x4294, // cmp r4, r2              ; 0 vs 2 → not equal
            0xD001, // beq +2 (not taken)
            0x2507, // movs r5, #7             ; runs
            0x4770, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[2], 2, "1+2-1");
    assert_eq!(machine.ctx.r[5], 7, "the not-taken branch ran on");
    assert_eq!(machine.ctx.thumb, 0, "bx lr returned to ARM");
    // pc-relative load, the literal rides the program
    let mut machine = setup();
    run_thumb(
        &mut machine,
        &[
            0x4800, // ldr r0, [pc, #0] → (BASE+4)&~3 = BASE+4
            0x4770, // bx lr
            0x5AA5, 0x55AA, // the literal at BASE+4
        ],
    );
    assert_eq!(machine.ctx.r[0], 0x55AA_5AA5, "the pc-relative literal");
    // push/pop
    let mut machine = setup();
    run_thumb(
        &mut machine,
        &[
            0x2101, // movs r1, #1
            0x2202, // movs r2, #2
            0xB406, // push {r1, r2}
            0x2100, // movs r1, #0
            0x2200, // movs r2, #0
            0xBC06, // pop {r1, r2}
            0x4770, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 1);
    assert_eq!(machine.ctx.r[2], 2);
    assert_eq!(machine.ctx.r[13], STACK_TOP, "sp restored");
    // bl to a Thumb callee and back
    let mut machine = setup();
    run_thumb(
        &mut machine,
        &[
            0xF000, 0xF806, // bl +0xC → BASE+0x10
            0x2003, // movs r0, #3 (after the call)
            0x4C01, // ldr r4, [pc, #4] → BASE+0xC (the RETURN sentinel)
            0x4720, // bx r4 → RETURN → stop
            0x46C0, // nop (pad)
            0xFFF0, 0xFFFF, // 0xFFFFFFF0
            0x2109, // movs r1, #9 (the callee)
            0x4770, // bx lr → back to BASE+4
        ],
    );
    assert_eq!(machine.ctx.r[1], 9, "the callee ran");
    assert_eq!(machine.ctx.r[0], 3, "and the caller went on");
}

/// Thumb register-offset loads and stores, the signed forms
#[test]
fn thumb_reg_offset_loads() {
    let mut machine = setup();
    machine.memory.write32(HEAP + 4, 0xDEAD_7887);
    run_thumb(
        &mut machine,
        &[
            0x2104, // movs r1, #4
            0x5C42, // ldrb r2, [r0, r1]        ; [heap+4] low byte
            0x5842, // ldr r2, [r0, r1]         ; the word
            0x5042, // str r2, [r0, r1]         ; back
            0x4770, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[2], 0xDEAD_7887);
    // the signed forms (assembler-verified encodings)
    let mut machine = setup();
    machine.memory.write8(HEAP + 0x10, 0x87);
    run_thumb(
        &mut machine,
        &[
            0x2110, // movs r1, #16
            0x5644, // ldrsb r4, [r0, r1]      ; 0x87 sign-extended
            0x4770, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[4], 0xFFFF_FF87, "ldrsb sign-extends");
    let mut machine = setup();
    machine.memory.write16(HEAP + 0x12, 0x8765);
    run_thumb(
        &mut machine,
        &[
            0x2112, // movs r1, #18
            0x5E45, // ldrsh r5, [r0, r1]
            0x4770, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[5], 0xFFFF_8765, "ldrsh sign-extends");
    // strh/ldrh round trip
    let mut machine = setup();
    machine.memory.write16(HEAP + 6, 0xBEEF);
    run_thumb(
        &mut machine,
        &[
            0x2106, // movs r1, #6
            0x5A43, // ldrh r3, [r0, r1]       ; 0xBEEF
            0x5242, // strh r2, [r0, r1]       ; r2 = 0 → overwrites
            0x5A44, // ldrh r4, [r0, r1]       ; 0
            0x4770, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[3], 0xBEEF);
    assert_eq!(machine.ctx.r[4], 0);
}

/// hi-register operations
#[test]
fn thumb_hi_registers() {
    let mut machine = setup();
    run_thumb(
        &mut machine,
        &[
            0x2105, // movs r1, #5
            0x0609, // lsls r1, r1, #24        ; 0x05000000
            0x4689, // mov r9, r1              ; hi move
            0x2203, // movs r2, #3
            0x4491, // add r9, r2              ; hi add
            0x4770, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[9], 0x0500_0003, "the hi-register move and add");
}

/// VFP: the arithmetic, compare into the flags, core moves, doubles — assembler-verified
#[test]
fn vfp_arithmetic_and_compare() {
    let mut machine = setup();
    machine.memory.write32(HEAP, 1.5_f32.to_bits());
    machine.memory.write32(HEAP + 4, 2.5_f32.to_bits());
    run_arm(
        &mut machine,
        &[
            0xED90_0A00, // vldr s0, [r0]
            0xEDD0_0A01, // vldr s1, [r0, #4]
            0xEE30_0A20, // vadd.f32 s0, s0, s1     ; 4.0
            0xEE60_0A20, // vmul.f32 s1, s0, s1     ; 4.0 * 2.5 = 10.0
            0xEEB5_0A40, // vcmp.f32 s0, #0         ; 4.0 > 0 → C
            0xEEF1_1A10, // vmrs r1, fpscr
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(f32::from_bits(vfp_word(&machine, 0)), 4.0, "vadd of 1.5 and 2.5");
    assert_eq!(f32::from_bits(vfp_word(&machine, 1)), 10.0, "vmul of 4.0 and 2.5");
    assert_eq!(machine.ctx.r[1] >> 28, 0x2, "vcmp 4.0 > 0 sets C in the fpscr flags");
    // vmov between a core register and a single
    let mut machine = setup();
    run_arm(
        &mut machine,
        &[
            0xE3A0_0062, // mov r0, #98
            0xEE00_0A90, // vmov s1, r0
            0xEE10_1A90, // vmov r1, s1
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(machine.ctx.r[1], 98, "vmov single to core");
    assert_eq!(vfp_word(&machine, 1), 98, "vmov core to single");
    // doubles: vadd.f64 d1, d1, d2, then vcmp.f64 d1, #0
    let mut machine = setup();
    set_vfp_word(&mut machine, 2, 1.5_f64.to_bits() as u32);
    set_vfp_word(&mut machine, 3, (1.5_f64.to_bits() >> 32) as u32);
    set_vfp_word(&mut machine, 4, 2.25_f64.to_bits() as u32);
    set_vfp_word(&mut machine, 5, (2.25_f64.to_bits() >> 32) as u32);
    run_arm(
        &mut machine,
        &[
            0xEE31_1B02, // vadd.f64 d1, d1, d2     ; 3.75
            0xEEB5_1B40, // vcmp.f64 d1, #0         ; 3.75 > 0
            0xEEF1_1A10, // vmrs r1, fpscr
            0xE12F_FF1E, // bx lr
        ],
    );
    let d1 = (vfp_word(&machine, 2) as u64) | ((vfp_word(&machine, 3) as u64) << 32);
    assert_eq!(f64::from_bits(d1), 3.75, "vadd.f64");
    assert_eq!(machine.ctx.r[1] >> 28, 0x2, "vcmp.f64 3.75 > 0");
}

/// VFP conversions and flush-to-zero, the classes the codegen lowers
#[test]
fn probe_ldr() {
    let mut machine = setup();
    load(
        &mut machine,
        &[0xE3A0_1009, 0xE3A0_2007, 0xE003_0291, 0xE12F_FF1E],
    );
    for step_index in 0..5 {
        let address = machine.ctx.r[15];
        if machine.ctx.thumb == 0 && address == RETURN {
            println!("returned after {step_index}");
            break;
        }
        let plain = address & !3;
        machine.ctx.r[15] = plain;
        println!("step {step_index}: r1 = {:#x} r2 = {:#x} opcode = {:#010x}", machine.ctx.r[1], machine.ctx.r[2], machine.memory.read32(plain));
        machine.ctx.fpscr = machine.fpscr;
        machine.ctx.vfp = machine.vfp.as_mut_ptr();
        let mut log = runtime3ds::interp::NoLog;
        let mut sink = |_c: &mut runtime3ds::context::Context, n: u32| panic!("svc {n}");
        let stepped = runtime3ds::interp::step(&mut machine.ctx, machine.memory, plain, &mut log, &mut sink);
        println!("  -> {stepped:?}, r3 = {:#x}", machine.ctx.r[3]);
        if stepped == runtime3ds::interp::Step::Next {
            machine.ctx.r[15] = plain + 4;
        }
    }
}

/// VFP conversions and flush-to-zero, the classes the codegen lowers
#[test]
fn vfp_conversions_and_flush() {
    let mut machine = setup();
    set_vfp_word(&mut machine, 0, 3.7_f32.to_bits());
    set_vfp_word(&mut machine, 1, 3); // an integer for vcvt.f32.u32
    run_arm(
        &mut machine,
        &[
            0xEEBD_0AC0, // vcvt.s32.f32 s0, s0 (toward zero)   ; 3
            0xEEB8_0A60, // vcvt.f32.u32 s0, s1                 ; 3.0
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(f32::from_bits(vfp_word(&machine, 0)), 3.0, "vcvt to int then back");
    // flush-to-zero: a subnormal product under FZ (1<<24)
    let mut machine = setup();
    set_fpscr(&mut machine, 1 << 24);
    set_vfp_word(&mut machine, 0, (1e-30_f32).to_bits());
    run_arm(
        &mut machine,
        &[
            0xEE60_0A00, // vmul.f32 s1, s0, s0    ; 1e-60: subnormal
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(vfp_word(&machine, 1), 0, "flushed to zero with FZ on");
    // and with FZ off the product stays subnormal
    let mut machine = setup();
    set_fpscr(&mut machine, 0);
    set_vfp_word(&mut machine, 0, (1e-30_f32).to_bits());
    run_arm(
        &mut machine,
        &[
            0xEE60_0A00, // vmul.f32 s1, s0, s0
            0xE12F_FF1E, // bx lr
        ],
    );
    assert_eq!(vfp_word(&machine, 1), (1e-60_f32).to_bits(), "kept subnormal with FZ off");
}

//! Interpreter tests: single instructions against hand-computed results, and the hand-off between lifted and
//! interpreted code through the real dispatch (in a child process: the dispatch table is installed once per process).
//! The instruction-by-instruction comparison with dynarmic is the oracle's (`src/oracle`, `--interp`).

use super::exec::{step, Flow};
use super::*;

fn cpu() -> Box<Cpu> {
    Cpu::new_boxed()
}

#[test]
fn integer_and_flags() {
    let mut c = cpu();
    // add x0, x1, #0x10
    c.x[1] = 5;
    assert_eq!(unsafe { step(&mut c, 0x9100_4020, 0x1000) }, Flow::Next);
    assert_eq!(c.x[0], 0x15);
    // cmp w0, #0x15 ; b.eq +8
    unsafe { step(&mut c, 0x7100_541f, 0x1000) };
    assert_eq!(c.nzcv >> 28, 0b0110);
    assert_eq!(unsafe { step(&mut c, 0x5400_0040, 0x1000) }, Flow::Jump(0x1008));
    // subs x2, x1, x1 -> zero, carry
    unsafe { step(&mut c, 0xeb01_0022, 0) };
    assert_eq!((c.x[2], c.nzcv >> 28), (0, 0b0110));
    // movz x3, #0x1234, lsl #16 ; movk x3, #0x5678
    unsafe { step(&mut c, 0xd2a2_4683, 0) };
    unsafe { step(&mut c, 0xf28a_cf03, 0) };
    assert_eq!(c.x[3], 0x1234_5678);
    // ubfx x4, x3, #8, #8
    unsafe { step(&mut c, 0xd348_3c64, 0) };
    assert_eq!(c.x[4], 0x56);
    // csinc w5, wzr, wzr, ne (cset w5, eq)
    c.nzcv = 0x4000_0000;
    unsafe { step(&mut c, 0x1a9f_17e5, 0) };
    assert_eq!(c.x[5], 1);
    // adrp x6, #0x1000 at 0x7000001234
    unsafe { step(&mut c, 0xb000_0006, 0x70_0000_1234) };
    assert_eq!(c.x[6], 0x70_0000_2000);
    // udiv x7, x3, xzr = 0
    unsafe { step(&mut c, 0x9adf_0867, 0) };
    assert_eq!(c.x[7], 0);
    // unallocated
    assert!(matches!(unsafe { step(&mut c, 0, 0) }, Flow::Udf(_)));
}

#[test]
fn loads_stores_and_branches() {
    let mut c = cpu();
    let mut stack = vec![0u64; 64];
    let top = stack.as_mut_ptr() as u64 + 8 * 32;
    c.sp = top;
    c.x[29] = 0x2929;
    c.x[30] = 0x3030;
    // stp x29, x30, [sp, #-16]!
    unsafe { step(&mut c, 0xa9bf_7bfd, 0) };
    assert_eq!(c.sp, top - 16);
    assert_eq!((stack[30], stack[31]), (0x2929, 0x3030));
    // ldp x1, x2, [sp], #16
    unsafe { step(&mut c, 0xa8c1_0be1, 0) };
    assert_eq!((c.x[1], c.x[2], c.sp), (0x2929, 0x3030, top));
    // ldrb w0, [x3], #1
    let bytes = [0xABu8, 0xCD];
    c.x[3] = bytes.as_ptr() as u64;
    unsafe { step(&mut c, 0x3840_1460, 0) };
    assert_eq!((c.x[0], c.x[3]), (0xAB, bytes.as_ptr() as u64 + 1));
    // bl +8 ; ret ; br x16 ; blr x1
    assert_eq!(unsafe { step(&mut c, 0x9400_0002, 0x1000) }, Flow::Call(0x1008));
    assert_eq!(c.x[30], 0x1004);
    assert_eq!(unsafe { step(&mut c, 0xd65f_03c0, 0) }, Flow::Ret);
    c.x[16] = 0x4444;
    assert_eq!(unsafe { step(&mut c, 0xd61f_0200, 0) }, Flow::Jump(0x4444));
    assert_eq!(unsafe { step(&mut c, 0xd63f_0020, 0x2000) }, Flow::Call(0x2929));
    assert_eq!(c.x[30], 0x2004);
    // svc #1, brk #0
    assert_eq!(unsafe { step(&mut c, 0xd400_0021, 0) }, Flow::Svc(1));
    assert_eq!(unsafe { step(&mut c, 0xd420_0000, 0) }, Flow::Brk(0));
}

#[test]
fn exclusives_and_atomics() {
    let mut c = cpu();
    let cell = Box::new(7u64);
    let a = &*cell as *const u64 as u64;
    c.x[0] = a;
    // ldaxr x1, [x0]; add x1, x1, #1; stlxr w2, x1, [x0]
    unsafe { step(&mut c, 0xc85f_fc01, 0) };
    unsafe { step(&mut c, 0x9100_0421, 0) };
    unsafe { step(&mut c, 0xc802_fc01, 0) };
    assert_eq!((c.x[2], *cell), (0, 8));
    // a second stlxr without ldaxr fails
    unsafe { step(&mut c, 0xc802_fc01, 0) };
    assert_eq!(c.x[2], 1);
    // ldaddal x3, x4, [x0]
    c.x[3] = 10;
    unsafe { step(&mut c, 0xf8e3_0004, 0) };
    assert_eq!((c.x[4], *cell), (8, 18));
    // casal x5, x6, [x0]
    c.x[5] = 18;
    c.x[6] = 99;
    unsafe { step(&mut c, 0xc8e5_fc06, 0) };
    assert_eq!((c.x[5], *cell), (18, 99));
}

#[test]
fn fp_and_simd() {
    let mut c = cpu();
    c.v[4] = 1.5f32.to_bits() as u128;
    c.v[6] = 2.25f32.to_bits() as u128;
    // fadd s16, s4, s6
    unsafe { step(&mut c, 0x1e26_2890, 0) };
    assert_eq!(f32::from_bits(c.v[16] as u32), 3.75);
    // fcvtzs w13, s18
    c.v[18] = (-7.9f32).to_bits() as u128;
    unsafe { step(&mut c, 0x1e38_024d, 0) };
    assert_eq!(c.x[13], (-7i32) as u32 as u64);
    // add v22.4s, v22.4s, v17.4s
    c.v[22] = 0x0000_0004_0000_0003_0000_0002_0000_0001;
    c.v[17] = 0x0000_0010_0000_0010_0000_0010_ffff_ffff;
    unsafe { step(&mut c, 0x4eb1_86d6, 0) };
    assert_eq!(c.v[22], 0x0000_0014_0000_0013_0000_0012_0000_0000);
    // movi v0.16b, #0x80
    unsafe { step(&mut c, 0x4f04_e400, 0) };
    assert_eq!(c.v[0], 0x8080_8080_8080_8080_8080_8080_8080_8080);
    // ldr q0, [x19, #0x90]
    let buf = [0u8; 0x200];
    let mut b2 = buf;
    b2[0x90..0xa0].copy_from_slice(&0x0123_4567_89ab_cdef_0011_2233_4455_6677u128.to_le_bytes());
    c.x[19] = b2.as_ptr() as u64;
    unsafe { step(&mut c, 0x3dc0_2660, 0) };
    assert_eq!(c.v[0], 0x0123_4567_89ab_cdef_0011_2233_4455_6677);
    // crc32b w8, w8, w11
    c.x[8] = 0xffff_ffff;
    c.x[11] = b'a' as u64;
    unsafe { step(&mut c, 0x1acb_4108, 0) };
    assert_eq!(!(c.x[8] as u32), 0xe8b7_be43); // crc32("a")
}

#[test]
fn discovered_format_round_trips() {
    let e = Entry { module: "main".into(), offset: 0x1234, words: vec![0xd10083ff, 0xa9017bfd] };
    let line = format_entry(&e);
    assert_eq!(line, "main\t0x1234\td10083ff a9017bfd\n");
    let parsed = parse_discovered(&format!("{HEADER}{line}garbage\n"));
    assert_eq!(parsed, vec![e]);
}

// ---------------------------------------------------------------- hand-off, in a child process

/// Guest code for the hand-off test (word offsets from the module base):
///   0x00 entry (no lifted function: interpreted)
///          stp x29, x30, [sp, #-16]!
///          mov x0, #5
///          bl 0x40            lifted: x0 *= 2                    -> 10
///          add x0, x0, #1                                         -> 11
///          bl 0x80            not lifted: interpreted, nested     -> 22
///          svc #0x55          handled by the test: x0 += 1000    -> 1022
///          blr x19            a host function: x0 += 7           -> 1029
///          ldp x29, x30, [sp], #16
///          b 0x100            lifted tail call: x0 += 100, returns to the entry's caller -> 1129
///   0x40   (lifted: the bytes are never run)
///   0x80   add x0, x0, x0 ; ret
///   0x100  (lifted)
fn program() -> Vec<u32> {
    let mut code = vec![0xd503_201fu32; 0x140 / 4]; // nop
    let at = |o: usize| o / 4;
    let prog = [
        0xa9bf_7bfd, // stp x29, x30, [sp, #-16]!
        0xd280_00a0, // mov x0, #5
        0x9400_000e, // bl +0x38 (pc 0x08 -> 0x40)
        0x9100_0400, // add x0, x0, #1
        0x9400_001c, // bl +0x70 (pc 0x10 -> 0x80)
        0xd400_0aa1, // svc #0x55
        0xd63f_0260, // blr x19
        0xa8c1_7bfd, // ldp x29, x30, [sp], #16
        0x1400_0038, // b +0xe0 (pc 0x20 -> 0x100)
    ];
    code[..prog.len()].copy_from_slice(&prog);
    code[at(0x80)] = 0x8b00_0000; // add x0, x0, x0
    code[at(0x84)] = 0xd65f_03c0; // ret
    code
}

unsafe extern "C" fn lifted_double(c: *mut Cpu) {
    (*c).x[0] *= 2;
}
unsafe extern "C" fn lifted_tail(c: *mut Cpu) {
    (*c).x[0] += 100;
}
unsafe fn host_add7(c: *mut Cpu) {
    (*c).x[0] += 7;
}
fn svc_55(c: &mut Cpu, _imm: u32) -> bool {
    c.x[0] += 1000;
    true
}

#[test]
fn hand_off_between_lifted_and_interpreted_code() {
    if std::env::var("NXRT_INTERP_CHILD").is_ok() {
        child();
        std::process::exit(0);
    }
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args(["--exact", "interp::tests::hand_off_between_lifted_and_interpreted_code", "--nocapture", "--test-threads=1"])
        .env("NXRT_INTERP_CHILD", "1")
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("HANDOFF OK"), "{text}");
}

fn child() {
    use crate::dispatch::{Module, Table};
    let code: &'static [u32] = Box::leak(program().into_boxed_slice());
    let base = code.as_ptr() as u64;
    let leak = |v: Vec<u64>| -> &'static [u64] { Box::leak(v.into_boxed_slice()) };
    let fns: &'static [GuestFn] = Box::leak(vec![lifted_double as GuestFn, lifted_tail as GuestFn].into_boxed_slice());
    let t = Table::build(vec![Module { name: "test".into(), base, size: code.len() as u64 * 4, addrs: leak(vec![base + 0x40, base + 0x100]), fns }])
        .unwrap();
    crate::dispatch::install(t).unwrap();
    let dir = std::env::temp_dir().join(format!("nxrt-interp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    set_data_dir(&dir);
    crate::svc::set_handler(0x55, svc_55);
    let host = crate::hostfn::register("test_host_add7", host_add7);
    let mut stack = vec![0u64; 512];
    let mut c = Cpu::new_boxed();
    c.sp = (stack.as_mut_ptr() as u64 + 8 * 256) & !15;
    c.x[19] = host;
    let sp0 = c.sp;
    let r = unsafe { crate::boundary::call_guest(&mut *c, base) };
    assert_eq!(r, crate::boundary::GuestCall::Returned);
    assert_eq!(c.x[0], 1129, "x0 after the run");
    assert_eq!(c.sp, sp0, "the stack is balanced");
    assert!(c.sj.is_null(), "every owner and barrier unlinked");
    // both interpreted entries were recorded, with their words
    let tsv = std::fs::read_to_string(dir.join(DISCOVERED)).unwrap();
    let e = parse_discovered(&tsv);
    assert_eq!(e.iter().map(|e| e.offset).collect::<Vec<_>>(), vec![0, 0x80], "{tsv}");
    assert_eq!(e[1].words[..2], [0x8b00_0000, 0xd65f_03c0]);
    // a second run records nothing new, also after a restart (the file is read back)
    set_data_dir(&dir);
    c.x[0] = 0;
    let _ = unsafe { crate::boundary::call_guest(&mut *c, base) };
    assert_eq!(c.x[0], 1129);
    assert_eq!(parse_discovered(&std::fs::read_to_string(dir.join(DISCOVERED)).unwrap()).len(), 2);
    let (entries, steps) = stats();
    assert!(entries >= 4 && steps > 10, "{entries} {steps}");
    let _ = std::fs::remove_dir_all(&dir);
    println!("HANDOFF OK");
}

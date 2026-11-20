//! nx-oracle: the translator's lifted C against dynarmic (docs/PORT_PLAN.md, T-lane).
//!
//!     nx-oracle [--cases 2000] [--states 8] [--seed N] [--class a,b] [--out DIR] [--jobs 4] [--cc CLANG] [--report-only]
//!               [--fpcr HEX] [--words HEX,HEX,...]
//!
//! `--fpcr`: every state starts with this FPCR (and the host MXCSR set from it as the runtime sets it: FTZ|DAZ =
//! FPCR.FZ), on both sides. `--words`: one class `words` of exactly these instruction words (no other class),
//! each from `--states` random states (no memory operands).
//!
//! Per instruction class (classes.rs): random words of the class's encoding group that the lifter lowers
//! (`Lifter::lift`, the raw per-instruction C before the function-local flag and register passes) are compiled into
//! one DLL with the generated code's headers and flags; each word then runs from `--states` random register / vector /
//! flag / memory states on both sides, the lifted C on the host and dynarmic (accurate settings) on its own memory
//! at the same addresses, and every architectural result is compared: X0-X30, SP, the next PC, NZCV, V0-V31,
//! TPIDR_EL0 and the data and code windows. FPSR's cumulative flags are counted apart (the portable C does not track
//! them all). Each class runs in a child process, so a lifted word that faults is reported with its word, not lost.
//! Output: one line per class and `<out>/mismatches.tsv`; exit 1 when a class mismatches or crashes.
//!
//! `--interp`: the runtime's interpreter (`nxrt::interp`, code the build missed) instead of the lifted C, on the same
//! words and states: every word the lifter lowers must run the same in the interpreter as in dynarmic (the next PC of a
//! branch from the interpreter's flow: a jump or call target, x30 for RET).

mod classes;
mod dynarmic;

use classes::{Class, Mem, CLASSES};
use dynarmic::{Context64, Jit};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use translator::xlate::{Kind, Lifter};

const DATA: u64 = 0x10_0000_0000;
const DATA_SIZE: u64 = 1 << 20;
const CODE: u64 = 0x20_0000_0000;
const CODE_SIZE: u64 = 4 << 20;
const PC: u64 = CODE + (2 << 20);
const BRK: u32 = 0xD420_0000;

/// The guest CPU state of the generated code (translator runtime/cpu.h), 1024 bytes.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Cpu {
    x: [u64; 32],
    sp: u64,
    pc: u64,
    nzcv: u64,
    tpidr: u64,
    tpidrro: u64,
    excl_addr: u64,
    excl_val: u64,
    thread: u64,
    fpcr: u64,
    fpsr: u64,
    excl_val_hi: u64,
    sj: u64,
    excl_seq: u32,
    _pad: [u8; 512 - 356],
    v: [u128; 32],
}
const _: () = assert!(core::mem::size_of::<Cpu>() == 1024);

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    /// Integers that find edge cases: zero, one, all ones, the sign and overflow boundaries, small, 32-bit, random.
    fn gpr(&mut self) -> u64 {
        const EDGE: [u64; 10] = [0, 1, u64::MAX, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0x8000_0000_0000_0000, 0x7FFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF_8000_0000, 2];
        match self.below(10) {
            0 | 1 => EDGE[self.below(EDGE.len() as u64) as usize],
            2 => self.below(256),
            3 => self.next() as u32 as u64,
            4 => self.next() as u32 as i32 as i64 as u64,
            _ => self.next(),
        }
    }
    /// A 64-bit lane: random bits, or a float / double edge value (zeros, infinities, NaNs with payloads,
    /// denormals, ones, halves, values near the integer limits).
    fn lane(&mut self) -> u64 {
        const F32: [u32; 14] = [0, 0x8000_0000, 0x7F80_0000, 0xFF80_0000, 0x7FC0_0000, 0x7F80_0001, 0xFFC1_2345, 0x0000_0001, 0x807F_FFFF, 0x3F80_0000, 0xBF00_0000, 0x4F00_0000, 0xCF00_0000, 0x4B80_0000];
        const F64: [u64; 12] = [0, 0x8000_0000_0000_0000, 0x7FF0_0000_0000_0000, 0xFFF0_0000_0000_0000, 0x7FF8_0000_0000_0000, 0x7FF0_0000_0000_0001, 0x0000_0000_0000_0001, 0x3FF0_0000_0000_0000, 0xBFE0_0000_0000_0000, 0x43E0_0000_0000_0000, 0xC3E0_0000_0000_0000, 0x41E0_0000_0000_0000];
        match self.below(8) {
            0 | 1 => F64[self.below(F64.len() as u64) as usize],
            2 | 3 => {
                let a = F32[self.below(F32.len() as u64) as usize] as u64;
                let b = F32[self.below(F32.len() as u64) as usize] as u64;
                a | b << 32
            }
            4 => {
                // a normal double in a modest range
                let f = ((self.next() >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 2f64.powi(self.below(80) as i32 - 40);
                f.to_bits()
            }
            5 => {
                let f = |r: &mut Rng| ((((r.next() >> 40) as f32) / (1u32 << 24) as f32 - 0.5) * 2f32.powi(r.below(40) as i32 - 20)).to_bits() as u64;
                f(self) | f(self) << 32
            }
            _ => self.next(),
        }
    }
}

struct Opts {
    cases: usize,
    states: usize,
    seed: u64,
    only: Vec<String>,
    out: PathBuf,
    jobs: usize,
    cc: Option<PathBuf>,
    report_only: bool,
    /// Self-test: every lowered word gets a wrong extra statement; every class must then mismatch.
    mutate: bool,
    fpcr: u64,
    words: Vec<u32>,
    /// Run the interpreter, not the lifted C.
    interp: bool,
}

fn parse() -> (Opts, Option<String>) {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let val = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1).cloned());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let o = Opts {
        cases: val("--cases").and_then(|v| v.parse().ok()).unwrap_or(2000),
        states: val("--states").and_then(|v| v.parse().ok()).unwrap_or(8),
        seed: val("--seed").and_then(|v| v.parse().ok()).unwrap_or(0x5EED),
        only: val("--class").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default(),
        out: val("--out").map(PathBuf::from).unwrap_or_else(|| root.join("build/Q/oracle")),
        jobs: val("--jobs").and_then(|v| v.parse().ok()).unwrap_or(4),
        cc: val("--cc").map(PathBuf::from),
        report_only: a.iter().any(|x| x == "--report-only"),
        mutate: a.iter().any(|x| x == "--mutate"),
        fpcr: val("--fpcr").and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok()).unwrap_or(0),
        words: val("--words").map(|v| v.split(',').filter_map(|w| u32::from_str_radix(w.trim().trim_start_matches("0x"), 16).ok()).collect()).unwrap_or_default(),
        interp: a.iter().any(|x| x == "--interp"),
    };
    (o, val("--child"))
}

/// clang for the test DLLs: llvm-mingw (`NXRS_LLVM_MINGW`, the engine's build\tools\llvm-mingw, PATH), else clang.
fn find_cc(o: &Opts) -> PathBuf {
    if let Some(c) = &o.cc {
        return c.clone();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut dirs: Vec<PathBuf> = vec![];
    if let Ok(d) = std::env::var("NXRS_LLVM_MINGW") {
        dirs.push(PathBuf::from(d));
    }
    dirs.push(root.join("build/tools/llvm-mingw"));
    for d in dirs {
        let p = d.join("bin/x86_64-w64-mingw32-clang.exe");
        if p.is_file() {
            return p;
        }
    }
    if Command::new("x86_64-w64-mingw32-clang").arg("--version").output().is_ok() {
        return "x86_64-w64-mingw32-clang".into();
    }
    "clang".into()
}

/// The words of a class: drawn until `n` are lowered by the lifter (not a trap, no runtime call).
fn draw(class: &Class, n: usize, rng: &mut Rng, exact: &[u32]) -> Vec<(u32, translator::xlate::Insn)> {
    let lifter = Lifter::new();
    if !exact.is_empty() {
        return exact.iter().filter_map(|&w| lifter.lift(w, PC).ok().filter(|i| i.kind != Kind::Trap).map(|i| (w, i))).collect();
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for _ in 0..n * 400 {
        if out.len() >= n {
            break;
        }
        let w = if class.words.is_empty() {
            (rng.next() as u32 & !class.mask) | class.value
        } else {
            class.words[rng.below(class.words.len() as u64) as usize] | (rng.next() as u32 & 31)
        };
        if !seen.insert(w) || classes::unpredictable(class, w) {
            continue;
        }
        let Ok(insn) = lifter.lift(w, PC) else { continue };
        if insn.kind == Kind::Trap {
            continue;
        }
        // runtime calls (svc, traps, dispatch, the counter) are the runtime's, not the lifter's; PACGA's value depends
        // on the keys (implementation defined), so it cannot equal dynarmic's
        const RUNTIME: [&str; 9] = ["rt_svc", "rt_udf", "rt_brk", "rt_dispatch", "rt_spin_gate", "rt_sj_owner", "rt_ticks", "fnprof", "rt_pac"];
        if RUNTIME.iter().any(|r| insn.code.contains(r)) {
            continue;
        }
        // RET Xn with n != 30 is lowered as a return to the caller (the host-stack call model of mkw-recomp's
        // generator: a function returns to whoever called it); the architectural "pc = Xn" differs by design
        if insn.kind == Kind::Ret && (w >> 10) & 0x3F == 0 && classes::rn(w) != 30 {
            continue;
        }
        // a branch to itself would look like dynarmic refusing the word
        if matches!(insn.kind, Kind::B | Kind::Bl | Kind::Bcond) && insn.target == PC {
            continue;
        }
        out.push((w, insn));
    }
    out
}

/// The C of one class: a function per word, ending in the next pc, and the table the harness calls.
fn emit(words: &[(u32, translator::xlate::Insn)], mutate: bool) -> String {
    let mut s = String::from(
        "#include \"cpu.h\"\n\
         uint32_t rt_tick_hz = 19200000; volatile int g_fnprof; uint8_t rt_patch_on[1]; uint32_t *rt_excl_mon;\n\
         void fnprof_hit(uint64_t a, uint64_t b, uint64_t c) { (void)a; (void)b; (void)c; }\n\
         void rt_dispatch(Cpu *c) { (void)c; } void rt_sj_owner(Cpu *c, GuestFn b, uint64_t l, uint64_t h) { (void)c; (void)b; (void)l; (void)h; }\n\
         void rt_spin_gate(Cpu *c, uint64_t h) { (void)c; (void)h; } void rt_svc(Cpu *c, uint32_t i, uint64_t p) { (void)c; (void)i; (void)p; }\n\
         void rt_brk(Cpu *c, uint32_t i, uint64_t p) { (void)c; (void)i; (void)p; for (;;) __builtin_trap(); }\n\
         void rt_udf(Cpu *c, uint64_t p) { (void)c; (void)p; for (;;) __builtin_trap(); }\n\
         uint64_t rt_ticks(void) { return 0; }\n",
    );
    let next = PC + 4;
    for (i, (w, insn)) in words.iter().enumerate() {
        let tail = match insn.kind {
            Kind::Op => format!("c->pc = {next:#x}ull;"),
            Kind::B => format!("c->pc = {:#x}ull;", insn.target),
            Kind::Bl => format!("c->x[30] = {next:#x}ull; c->pc = {:#x}ull;", insn.target),
            Kind::Bcond => format!("c->pc = ({}) ? {:#x}ull : {next:#x}ull;", insn.cond, insn.target),
            Kind::Br => format!("c->pc = {};", insn.target_reg),
            Kind::Blr => format!("{{ uint64_t t = {}; c->x[30] = {next:#x}ull; c->pc = t; }}", insn.target_reg),
            Kind::Ret => "c->pc = c->x[30];".into(),
            Kind::Trap => unreachable!(),
        };
        let wrong = if mutate { " c->x[(c->pc >> 2) & 15] ^= 0x10;" } else { "" };
        let _ = writeln!(s, "/* {w:08x} */ static void t{i}(Cpu *c) {{ {} {tail}{wrong} }}", insn.code);
    }
    let _ = writeln!(s, "__declspec(dllexport) const GuestFn oracle_fns[] = {{");
    for i in 0..words.len() {
        let _ = writeln!(s, "  t{i},");
    }
    let _ = writeln!(s, "  0 }};\n__declspec(dllexport) const int oracle_count = {};", words.len());
    s
}

fn compile(cc: &Path, c: &Path, dll: &Path, include: &Path) -> Result<(), String> {
    let mut cmd = Command::new(cc);
    cmd.args(["-std=gnu11", "-O1", "-fno-strict-aliasing", "-march=x86-64-v3", "-fno-vectorize", "-fno-slp-vectorize", "-fomit-frame-pointer", "-g0", "-w", "-shared", "-o"])
        .arg(dll)
        .arg(c)
        .arg("-I")
        .arg(include);
    if !cc.to_string_lossy().contains("mingw32") {
        cmd.arg("-fuse-ld=lld");
    }
    let o = cmd.output().map_err(|e| format!("{}: {e}", cc.display()))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    }
}

fn alloc(addr: u64, size: u64) -> &'static mut [u8] {
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};
    // SAFETY: a fixed reservation the process keeps; checked for the address asked
    let p = unsafe { VirtualAlloc(addr as *const _, size as usize, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
    assert_eq!(p as u64, addr, "VirtualAlloc at {addr:#x}");
    // SAFETY: committed above, owned for the process lifetime
    unsafe { std::slice::from_raw_parts_mut(p as *mut u8, size as usize) }
}

/// One random starting state for a word of a class: registers, flags, vectors; addresses where the class needs them.
fn state(class: &Class, w: u32, rng: &mut Rng, fpcr: u64) -> Cpu {
    // SAFETY: Cpu is plain data
    let mut c: Cpu = unsafe { core::mem::zeroed() };
    for i in 0..31 {
        c.x[i] = rng.gpr();
    }
    c.nzcv = (rng.below(16)) << 28;
    for i in 0..32 {
        c.v[i] = rng.lane() as u128 | (rng.lane() as u128) << 64;
    }
    c.fpcr = fpcr;
    c.tpidr = rng.next();
    c.tpidrro = rng.next();
    let mid = DATA + DATA_SIZE / 2;
    c.sp = (mid + rng.below(512) * 16).wrapping_sub(4096);
    c.pc = PC;
    let base = |rng: &mut Rng, align: u64| (mid + rng.below(512)).wrapping_sub(256) & !(align - 1);
    let n = classes::rn(w) as usize;
    match class.mem {
        Mem::None | Mem::Literal => {}
        Mem::Base { align, .. } => {
            let b = base(rng, align);
            if n == 31 { c.sp = b & !15 } else { c.x[n] = b }
        }
        Mem::BaseIndex { .. } => {
            let b = base(rng, 16);
            if n == 31 { c.sp = b & !15 } else { c.x[n] = b }
            let m = classes::rm(w) as usize;
            if m != 31 {
                // small and positive: UXTW / LSL of a negative offset would leave the window
                c.x[m] = rng.below(65);
            }
        }
    }
    c
}

fn context(c: &Cpu) -> Context64 {
    let mut x = [0u64; 31];
    x.copy_from_slice(&c.x[..31]);
    Context64 {
        sp: c.sp,
        pc: c.pc,
        x,
        v: core::array::from_fn(|i| [c.v[i] as u64, (c.v[i] >> 64) as u64]),
        fpcr: c.fpcr as u32,
        fpsr: c.fpsr as u32,
        pstate: c.nzcv as u32 & 0xF000_0000,
        tpidr_el0: c.tpidr,
        tpidrro_el0: c.tpidrro,
    }
}

/// The fields that differ: (name, before, lifted C, dynarmic).
fn compare(before: &Cpu, l: &Cpu, d: &Context64) -> (Vec<(String, String, String, String)>, bool) {
    let mut out = vec![];
    let mut put = |name: String, b: String, x: String, y: String| out.push((name, b, x, y));
    for i in 0..31 {
        if l.x[i] != d.x[i] {
            put(format!("x{i}"), format!("{:#x}", before.x[i]), format!("{:#x}", l.x[i]), format!("{:#x}", d.x[i]));
        }
    }
    if l.sp != d.sp {
        put("sp".into(), format!("{:#x}", before.sp), format!("{:#x}", l.sp), format!("{:#x}", d.sp));
    }
    if l.pc != d.pc {
        put("pc".into(), format!("{:#x}", before.pc), format!("{:#x}", l.pc), format!("{:#x}", d.pc));
    }
    if (l.nzcv as u32 & 0xF000_0000) != (d.pstate & 0xF000_0000) {
        put("nzcv".into(), format!("{:x}", before.nzcv >> 28), format!("{:x}", (l.nzcv >> 28) & 15), format!("{:x}", d.pstate >> 28));
    }
    for i in 0..32 {
        let dv = d.v[i][0] as u128 | (d.v[i][1] as u128) << 64;
        if l.v[i] != dv {
            put(format!("v{i}"), format!("{:032x}", before.v[i]), format!("{:032x}", l.v[i]), format!("{dv:032x}"));
        }
    }
    if l.tpidr != d.tpidr_el0 {
        put("tpidr".into(), format!("{:#x}", before.tpidr), format!("{:#x}", l.tpidr), format!("{:#x}", d.tpidr_el0));
    }
    let fpsr = (l.fpsr as u32) != d.fpsr;
    (out, fpsr)
}

/// Runs one class (in a child process). Returns the summary line.
fn run_class(o: &Opts, class: &Class) -> String {
    let dir = o.out.join(class.name);
    let _ = std::fs::create_dir_all(&dir);
    let mut rng = Rng(o.seed ^ class.value as u64 ^ (class.mask as u64) << 32);
    // at most 2,500 words a class (one DLL each): a larger run (the 1 M nightly) takes more states per word instead
    let states = o.states.max(o.cases.div_ceil(2500));
    let nwords = if o.words.is_empty() { o.cases.div_ceil(states).max(1) } else { o.words.len() };
    let data = alloc(DATA, DATA_SIZE);
    let code = alloc(CODE, CODE_SIZE);
    for b in data.iter_mut().chain(code.iter_mut()) {
        *b = rng.next() as u8;
    }
    let jit = Jit::new();
    jit.map(DATA, DATA_SIZE);
    jit.map(CODE, CODE_SIZE);
    jit.write(DATA, data);
    jit.write(CODE, code);
    let off = (PC - CODE) as usize;
    let place = |code: &mut [u8], w: u32| {
        code[off..off + 4].copy_from_slice(&w.to_le_bytes());
        code[off + 4..off + 8].copy_from_slice(&BRK.to_le_bytes());
        jit.write(PC, &code[off..off + 8]);
        jit.invalidate(PC, 8);
    };
    // words the lifter lowers, then those dynarmic runs (an exception at the word: unallocated or not modelled there)
    let mut words = vec![];
    let mut refused_words = vec![];
    for (w, insn) in draw(class, nwords * 40, &mut rng, &o.words) {
        if words.len() >= nwords {
            break;
        }
        place(code, w);
        let probe = state(class, w, &mut rng, o.fpcr);
        if jit.run(&context(&probe), 1).pc == PC {
            refused_words.push(w);
        } else {
            words.push((w, insn));
        }
        jit.write(DATA, data);
    }
    let refused = refused_words.len();
    if words.is_empty() {
        return format!("RESULT\t{}\t0\t{refused}\t0\t0\t0\t0\tno word lowered and run", class.name);
    }
    // the lifted C in a DLL, or (--interp) the runtime's interpreter
    let mut _lib = None;
    let fns: &[extern "C" fn(*mut Cpu)] = if o.interp {
        &[]
    } else {
        let include = dir.join("inc");
        translator::aot::runtime_headers::write(&include).expect("headers");
        let c = dir.join("t.c");
        std::fs::write(&c, emit(&words, o.mutate)).unwrap();
        let dll = dir.join("t.dll");
        if let Err(e) = compile(&find_cc(o), &c, &dll, &include.join("include")) {
            std::fs::write(dir.join("compile.log"), &e).unwrap();
            return format!("RESULT\t{}\t{}\t0\t0\t0\t0\t0\tcompile failed (compile.log)", class.name, words.len());
        }
        // SAFETY: our own DLL; the table is an array of oracle_count functions
        let lib = _lib.insert(unsafe { libloading::Library::new(&dll) }.expect("load dll"));
        unsafe {
            let n = **lib.get::<*const i32>(b"oracle_count\0").unwrap();
            let p = *lib.get::<*const extern "C" fn(*mut Cpu)>(b"oracle_fns\0").unwrap();
            std::slice::from_raw_parts(p, n as usize)
        }
    };
    let mem_class = class.mem != Mem::None;
    let mut dmem = vec![0u8; DATA_SIZE as usize];

    let (mut cases, mut bad_cases, mut fpsr_diffs, mut faults) = (0usize, 0usize, 0usize, 0usize);
    let mut bad_words = std::collections::BTreeSet::new();
    let mut tsv = String::new();
    for (i, (w, insn)) in words.iter().enumerate() {
        eprintln!("at\t{w:08x}");
        place(code, *w);
        for s in 0..states {
            let before = state(class, *w, &mut rng, o.fpcr);
            let ctx = context(&before);
            let d = jit.run(&ctx, 1);
            if d.pc == PC {
                // an exception for this state only (an alignment fault, for one): nothing to compare
                faults += 1;
                continue;
            }
            let mut l = before;
            let old = mxcsr_enter(o.fpcr);
            if o.interp {
                interp_step(&mut l, *w);
            } else {
                fns[i](&mut l);
            }
            mxcsr_leave(old);
            cases += 1;
            let (mut diffs, fpsr) = compare(&before, &l, &d);
            fpsr_diffs += fpsr as usize;
            if mem_class {
                jit.read(DATA, &mut dmem);
                if dmem.as_slice() != &data[..] {
                    let first = dmem.iter().zip(data.iter()).position(|(a, b)| a != b).unwrap();
                    let a = DATA + first as u64;
                    let rd = |m: &[u8]| m[first..(first + 16).min(m.len())].iter().map(|b| format!("{b:02x}")).collect::<String>();
                    diffs.push((format!("mem@{a:#x}"), String::new(), rd(data), rd(&dmem)));
                    jit.write(DATA, data);
                }
            }
            if !diffs.is_empty() {
                bad_cases += 1;
                bad_words.insert(*w);
                for (f, b, x, y) in diffs.iter().take(6) {
                    let _ = writeln!(tsv, "{}\t{w:08x}\t{s}\t{f}\t{b}\t{x}\t{y}\t{}", class.name, insn.code.replace(['\t', '\n'], " "));
                }
            }
        }
    }
    std::fs::write(dir.join("mismatches.tsv"), &tsv).unwrap();
    std::fs::write(dir.join("refused.txt"), refused_words.iter().map(|w| format!("{w:08x}\n")).collect::<String>()).unwrap();
    format!(
        "RESULT\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        class.name,
        words.len(),
        refused,
        cases,
        bad_cases,
        bad_words.len(),
        fpsr_diffs,
        if faults > 0 { format!("{faults} states raised an exception in dynarmic") } else { String::new() }
    )
}

/// One word through the runtime's interpreter at PC; the next PC from its flow (a refused word leaves PC at the word,
/// which no dynarmic run does, so it counts as a mismatch).
fn interp_step(c: &mut Cpu, w: u32) {
    use nxrt::interp::exec::{step, Flow};
    // SAFETY: the oracle's Cpu is the runtime's (cpu.h) layout, size checked above
    let rc = unsafe { &mut *(c as *mut Cpu as *mut nxrt::cpu::Cpu) };
    // SAFETY: the data and code windows are mapped at the addresses the states use
    let next = match unsafe { step(rc, w, PC) } {
        Flow::Next => PC + 4,
        Flow::Jump(t) | Flow::Call(t) => t,
        Flow::Ret => rc.x[30],
        Flow::Svc(_) | Flow::Brk(_) | Flow::Udf(_) => PC,
    };
    c.pc = next;
}

/// The host MXCSR of a thread running guest code under `fpcr` (runtime `Cpu::sync_mxcsr`: FTZ|DAZ = FPCR.FZ, RC
/// nearest); returns the old value.
#[allow(deprecated)]
fn mxcsr_enter(fpcr: u64) -> u32 {
    use core::arch::x86_64::{_mm_getcsr, _mm_setcsr};
    // SAFETY: MXCSR control bits only
    unsafe {
        let cur = _mm_getcsr();
        _mm_setcsr((cur & !0xE040) | if fpcr & (1 << 24) != 0 { 0x8040 } else { 0 });
        cur
    }
}
#[allow(deprecated)]
fn mxcsr_leave(old: u32) {
    // SAFETY: restores the value read by mxcsr_enter
    unsafe { core::arch::x86_64::_mm_setcsr(old) }
}

static WORDS: Class = Class { name: "words", mask: 0, value: 0, mem: Mem::None, words: &[] };

fn main() {
    let (o, child) = parse();
    if let Some(name) = child {
        let class = if name == "words" { &WORDS } else { CLASSES.iter().find(|c| c.name == name).expect("class") };
        let line = run_class(&o, class);
        println!("{line}");
        std::io::stdout().flush().unwrap();
        // the JIT and the DLL go with the process
        std::process::exit(0);
    }
    let _ = std::fs::create_dir_all(&o.out);
    let classes: Vec<&Class> = if o.words.is_empty() {
        CLASSES.iter().filter(|c| o.only.is_empty() || o.only.iter().any(|n| n == c.name)).collect()
    } else {
        vec![&WORDS]
    };
    let exe = std::env::current_exe().unwrap();
    let t0 = std::time::Instant::now();
    let results = std::sync::Mutex::new(vec![]);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..o.jobs.max(1) {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(class) = classes.get(i) else { break };
                let mut cmd = Command::new(&exe);
                cmd.args(std::env::args().skip(1)).args(["--child", class.name]);
                let out = cmd.output().expect("child");
                let stdout = String::from_utf8_lossy(&out.stdout);
                let line = match stdout.lines().find(|l| l.starts_with("RESULT\t")) {
                    Some(l) => l.to_string(),
                    None => {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        let at = stderr.lines().rev().find_map(|l| l.strip_prefix("at\t")).unwrap_or("?").to_string();
                        format!("RESULT\t{}\t?\t?\t?\t?\t?\t?\tCRASH ({}) at word {at}", class.name, out.status)
                    }
                };
                results.lock().unwrap().push((i, line));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort();
    println!("{:<24} {:>6} {:>8} {:>7} {:>10} {:>10} {:>6}  note", "class", "words", "refused", "cases", "bad cases", "bad words", "fpsr");
    let (mut total, mut failed) = (0usize, 0usize);
    let mut all = String::from("class\tword\tstate\tfield\tbefore\tlifted\tdynarmic\tlifted C\n");
    for (_, line) in &results {
        let f: Vec<&str> = line.split('\t').collect();
        println!("{:<24} {:>6} {:>8} {:>7} {:>10} {:>10} {:>6}  {}", f[1], f[2], f[3], f[4], f[5], f[6], f[7], f.get(8).unwrap_or(&""));
        total += f[4].parse::<usize>().unwrap_or(0);
        let note = f.get(8).copied().unwrap_or("");
        if f[5] != "0" || note.starts_with("CRASH") || note.starts_with("compile failed") {
            failed += 1;
        }
        if let Ok(t) = std::fs::read_to_string(o.out.join(f[1]).join("mismatches.tsv")) {
            all.push_str(&t);
        }
    }
    std::fs::write(o.out.join("mismatches.tsv"), all).unwrap();
    println!(
        "{} classes, {total} cases in {:.1} s; {failed} class(es) with mismatches or crashes; details: {}",
        results.len(),
        t0.elapsed().as_secs_f64(),
        o.out.join("mismatches.tsv").display()
    );
    std::process::exit(if failed > 0 && !o.report_only { 1 } else { 0 });
}

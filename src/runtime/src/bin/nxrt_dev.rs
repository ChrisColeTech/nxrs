//! nxrt-dev: the build glue around the runtime, for development and the M2 homebrew tests.
//!
//!   nxrt-dev guest  --out <dir>                    build the homebrew tests: <dir>/<test>/exefs/{main,sdk}
//!   nxrt-dev link   --gen <dir> --out <exe>        compile translated C (+ modules.c) and link it with nxrt.lib
//!   nxrt-dev e2e    [--work <dir>] [--generator rust|mkw] [--nxrs <exe>] [test...]
//!                                                   guest build, translate, link, run, compare with <test>.expected
//!
//! The guest side is built with clang for aarch64-none-elf and ld.lld (linker script `guest/link.ld`), and each ELF
//! becomes an NSO (lz4-compressed segments, MOD0 from `guest/crt0.S`). The host side uses clang for
//! x86_64-pc-windows-msvc with the generated-code flag set the translated games use.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

type R<T> = Result<T, String>;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn llvm(tool: &str) -> PathBuf {
    if let Ok(d) = std::env::var("NXRT_LLVM") {
        return Path::new(&d).join(format!("{tool}.exe"));
    }
    let p = Path::new(r"C:\Program Files\LLVM\bin").join(format!("{tool}.exe"));
    if p.is_file() {
        p
    } else {
        tool.into()
    }
}

fn run(cmd: &mut Command) -> R<String> {
    let out = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !out.status.success() {
        return Err(format!(
            "{:?} {:?} failed:\n{}{}",
            cmd.get_program(),
            cmd.get_args().collect::<Vec<_>>(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs jobs (each a Command) on `n` threads; the first error wins.
fn parallel(cmds: Vec<Command>, n: usize) -> R<()> {
    let next = AtomicUsize::new(0);
    let cmds: Vec<Mutex<Command>> = cmds.into_iter().map(Mutex::new).collect();
    let err: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..n.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(c) = cmds.get(i) else { break };
                if err.lock().unwrap().is_some() {
                    break;
                }
                if let Err(e) = run(&mut c.lock().unwrap()) {
                    err.lock().unwrap().get_or_insert(e);
                }
            });
        }
    });
    match err.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------- guest build

const GUEST_C: &[&str] = &[
    "--target=aarch64-none-elf",
    "-march=armv8-a",
    "-mno-outline-atomics",
    "-O2",
    "-ffreestanding",
    "-fno-stack-protector",
    "-funwind-tables",
    "-fno-math-errno",
    "-fasynchronous-unwind-tables",
    "-Wall",
    "-Wno-unused-parameter",
];

fn guest_cc(src: &Path, obj: &Path, extra: &[&str]) -> Command {
    let mut c = Command::new(llvm("clang"));
    c.args(GUEST_C).args(extra).arg(format!("-I{}", root().join("guest/sdk").display())).arg("-c").arg(src).arg("-o").arg(obj);
    if src.extension().is_some_and(|e| e == "cpp") {
        c.args(["-std=c++17", "-fexceptions", "-fcxx-exceptions", "-nostdinc++", "-fno-threadsafe-statics", "-fno-use-cxa-atexit"]);
    }
    c
}

fn guest_ld(objs: &[PathBuf], libs: &[PathBuf], shared: bool, soname: &str, out: &Path) -> R<()> {
    let mut c = Command::new(llvm("ld.lld"));
    c.arg(if shared { "-shared" } else { "-pie" })
        .args(["--eh-frame-hdr", "--hash-style=sysv", "-z", "notext", "--build-id=sha1", "-soname", soname, "-T"])
        .arg(root().join("guest/link.ld"))
        .args(objs)
        .args(libs)
        .arg("-o")
        .arg(out);
    run(&mut c).map(|_| ())
}

fn rd<const N: usize>(b: &[u8], o: usize) -> [u8; N] {
    b[o..o + N].try_into().unwrap()
}

/// An ELF (from link.ld: three PT_LOADs at page-aligned addresses from 0) as an NSO with lz4 segments.
pub fn elf_to_nso(elf: &[u8]) -> R<Vec<u8>> {
    if elf.get(..4) != Some(b"\x7fELF") || elf[4] != 2 {
        return Err("not an ELF64".into());
    }
    let phoff = u64::from_le_bytes(rd(elf, 0x20)) as usize;
    let (phentsize, phnum) = (u16::from_le_bytes(rd(elf, 0x36)) as usize, u16::from_le_bytes(rd(elf, 0x38)) as usize);
    let mut loads = vec![];
    for i in 0..phnum {
        let p = phoff + i * phentsize;
        if u32::from_le_bytes(rd(elf, p)) != 1 {
            continue;
        }
        let off = u64::from_le_bytes(rd(elf, p + 8)) as usize;
        let vaddr = u64::from_le_bytes(rd(elf, p + 16)) as usize;
        let filesz = u64::from_le_bytes(rd(elf, p + 32)) as usize;
        let memsz = u64::from_le_bytes(rd(elf, p + 40)) as usize;
        loads.push((vaddr, elf[off..off + filesz].to_vec(), memsz));
    }
    loads.sort_by_key(|l| l.0);
    if loads.len() != 3 || loads[0].0 != 0 || loads.iter().any(|l| l.0 % 0x1000 != 0) {
        return Err(format!("expected three page-aligned PT_LOADs from 0, got {:?}", loads.iter().map(|l| (l.0, l.1.len())).collect::<Vec<_>>()));
    }
    let bss = loads[2].2 - loads[2].1.len();
    let mut h = vec![0u8; 0x100];
    h[..4].copy_from_slice(b"NSO0");
    h[0xC..0x10].copy_from_slice(&7u32.to_le_bytes());
    let mut body = vec![];
    for (i, (vaddr, data, _)) in loads.iter().enumerate() {
        let comp = lz4_flex::block::compress(data);
        let file_off = 0x100 + body.len();
        h[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&(file_off as u32).to_le_bytes());
        h[0x14 + 0x10 * i..0x18 + 0x10 * i].copy_from_slice(&(*vaddr as u32).to_le_bytes());
        h[0x18 + 0x10 * i..0x1C + 0x10 * i].copy_from_slice(&(data.len() as u32).to_le_bytes());
        h[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(comp.len() as u32).to_le_bytes());
        body.extend_from_slice(&comp);
    }
    h[0x3C..0x40].copy_from_slice(&(bss as u32).to_le_bytes());
    // build id: a hash of the segments
    let mut x: u64 = 0xcbf2_9ce4_8422_2325;
    for (_, d, _) in &loads {
        for &b in d {
            x = (x ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    h[0x40..0x48].copy_from_slice(&x.to_le_bytes());
    h.extend_from_slice(&body);
    Ok(h)
}

fn tests_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "expected"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Builds the SDK once and every test's main; writes <out>/<test>/exefs/{main,sdk}.
fn build_guest(out: &Path, tests: &[String]) -> R<()> {
    let g = root().join("guest");
    let obj = out.join("obj");
    std::fs::create_dir_all(&obj).map_err(|e| e.to_string())?;
    let o = |n: &str| obj.join(format!("{n}.o"));
    let mut cmds = vec![
        guest_cc(&g.join("crt0.S"), &o("crt0_sdk"), &[]),
        guest_cc(&g.join("crt0.S"), &o("crt0_main"), &["-DMAIN"]),
        guest_cc(&g.join("crt_main.c"), &o("crt_main"), &["-fPIE"]),
        guest_cc(&g.join("hostlib.c"), &o("hostlib"), &["-fPIC", "-w"]),
        guest_cc(&g.join("sdk/svc.s"), &o("svc"), &[]),
        guest_cc(&g.join("sdk/libc.c"), &o("libc"), &["-fPIC"]),
        guest_cc(&g.join("sdk/thread.c"), &o("thread"), &["-fPIC"]),
        guest_cc(&g.join("sdk/cxxrt.c"), &o("cxxrt"), &["-fPIC"]),
    ];
    let mut per_test: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for t in tests {
        let mut objs = vec![];
        for ext in ["c", "cpp"] {
            let src = g.join("tests").join(format!("{t}.{ext}"));
            if src.is_file() {
                cmds.push(guest_cc(&src, &o(&format!("t_{t}")), &["-fPIE"]));
                objs.push(o(&format!("t_{t}")));
            }
        }
        let extra = g.join("tests").join(format!("{t}_lse.c"));
        if extra.is_file() {
            cmds.push(guest_cc(&extra, &o(&format!("t_{t}_lse")), &["-fPIE", "-march=armv8.1-a+lse"]));
            objs.push(o(&format!("t_{t}_lse")));
        }
        if objs.is_empty() {
            return Err(format!("no source for test {t}"));
        }
        per_test.insert(t.clone(), objs);
    }
    parallel(cmds, 4)?;
    let sdk = obj.join("sdk.so");
    guest_ld(&[o("crt0_sdk"), o("svc"), o("libc"), o("thread"), o("cxxrt")], &[], true, "sdk", &sdk)?;
    let host = obj.join("host.so");
    guest_ld(&[o("crt0_sdk"), o("hostlib")], &[], true, "host", &host)?;
    let sdk_nso = elf_to_nso(&std::fs::read(&sdk).map_err(|e| e.to_string())?)?;
    for (t, objs) in per_test {
        let elf = obj.join(format!("{t}.elf"));
        let mut all = vec![o("crt0_main"), o("crt_main")];
        all.extend(objs);
        guest_ld(&all, &[sdk.clone(), host.clone()], false, "main", &elf)?;
        let exefs = out.join(&t).join("exefs");
        std::fs::create_dir_all(&exefs).map_err(|e| e.to_string())?;
        // a dump folder holds romfs/ beside exefs/ (the translator checks); homebrew has none, so it stays empty
        std::fs::create_dir_all(out.join(&t).join("romfs")).map_err(|e| e.to_string())?;
        std::fs::write(exefs.join("main"), elf_to_nso(&std::fs::read(&elf).map_err(|e| e.to_string())?)?).map_err(|e| e.to_string())?;
        std::fs::write(exefs.join("sdk"), &sdk_nso).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------- host build

const GEN_FLAGS: &[&str] = &[
    "-O1",
    "-fno-strict-aliasing",
    "-Wno-return-type",
    "-Wno-unused-label",
    "-march=x86-64-v3",
    "-fno-vectorize",
    "-fno-slp-vectorize",
    "-fomit-frame-pointer",
    "-g0",
    "-fms-runtime-lib=dll",
    "-w",
];

/// modules.c: the module table from layout.json and the generated tables, the profiler and patch switches the
/// generated code reads, and main() calling nxrt_main.
fn modules_c(gen: &Path) -> R<String> {
    let layout = std::fs::read_to_string(gen.join("layout.json")).map_err(|e| format!("layout.json: {e}"))?;
    // [{"name": "main", "base": "0x...", "size": "0x..."}, ...] (tiny parser: the translator writes this shape)
    let mut mods = vec![];
    let field = |chunk: &str, k: &str| -> Option<String> {
        let i = chunk.find(&format!("\"{k}\""))?;
        let rest = &chunk[i + k.len() + 2..];
        let a = rest.find('"')? + 1;
        let b = rest[a..].find('"')? + a;
        Some(rest[a..b].to_string())
    };
    for chunk in layout.split('{').skip(1) {
        let (Some(n), Some(b), Some(s)) = (field(chunk, "name"), field(chunk, "base"), field(chunk, "size")) else {
            return Err("layout.json: unexpected shape".into());
        };
        if gen.join(&n).join(format!("{n}_table.c")).is_file() {
            mods.push((n, b, s));
        }
    }
    let mut s = String::from(
        "// Generated by nxrt-dev: the translated modules for nxrt_main.\n#include \"nxrt.h\"\n\n\
         volatile int g_fnprof __attribute__((visibility(\"hidden\")));\n\
         uint8_t rt_patch_on[256] __attribute__((visibility(\"hidden\")));\n\n",
    );
    for (n, _, _) in &mods {
        s.push_str(&format!(
            "extern const uint64_t {n}_fn_addrs[];\nextern const GuestFn {n}_fn_ptrs[];\nextern const uint64_t {n}_fn_count;\n"
        ));
    }
    s.push_str("\nint main(int argc, char **argv) {\n    RtModule mods[] = {\n");
    for (n, b, z) in &mods {
        s.push_str(&format!("        {{\"{n}\", {b}ull, {z}ull, {n}_fn_addrs, {n}_fn_ptrs, 0}},\n"));
    }
    s.push_str("    };\n");
    for (i, (n, _, _)) in mods.iter().enumerate() {
        s.push_str(&format!("    mods[{i}].count = {n}_fn_count;\n"));
    }
    s.push_str(&format!("    return nxrt_main(argc, argv, mods, {});\n}}\n", mods.len()));
    Ok(s)
}

fn nxrt_lib() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().join("nxrt.lib")
}

fn link(gen: &Path, out: &Path) -> R<()> {
    let obj = gen.join("obj");
    std::fs::create_dir_all(&obj).map_err(|e| e.to_string())?;
    std::fs::write(obj.join("modules.c"), modules_c(gen)?).map_err(|e| e.to_string())?;
    let mut srcs = vec![obj.join("modules.c")];
    for e in std::fs::read_dir(gen).map_err(|e| e.to_string())?.filter_map(|e| e.ok()) {
        if e.path().is_dir() && e.path().join(format!("{}_table.c", e.file_name().to_string_lossy())).is_file() {
            for f in std::fs::read_dir(e.path()).map_err(|e| e.to_string())?.filter_map(|e| e.ok()) {
                if f.path().extension().is_some_and(|x| x == "c") {
                    srcs.push(f.path());
                }
            }
        }
    }
    // gen/include holds the headers the Rust translator writes; mkw-recomp's generator writes none, and the
    // translator's vendored copies (the same files) serve then.
    let incs = [gen.join("include"), root().join("../translator/runtime"), root().join("include")];
    let mut cmds = vec![];
    let mut objs = vec![];
    for (i, src) in srcs.iter().enumerate() {
        let o = obj.join(format!("{i:04}_{}.o", src.file_stem().unwrap().to_string_lossy()));
        let mut c = Command::new(llvm("clang"));
        c.args(GEN_FLAGS);
        for inc in &incs {
            c.arg(format!("-I{}", inc.display()));
        }
        c.arg("-c").arg(src).arg("-o").arg(&o);
        cmds.push(c);
        objs.push(o);
    }
    parallel(cmds, 4)?;
    let lib = nxrt_lib();
    if !lib.is_file() {
        return Err(format!("{} missing: cargo build -p runtime", lib.display()));
    }
    let mut c = Command::new(llvm("clang"));
    c.arg("-fuse-ld=lld")
        .arg("-fms-runtime-lib=dll")
        .args(&objs)
        .arg(&lib)
        .args(["-lkernel32", "-lntdll", "-luserenv", "-lws2_32", "-ldbghelp", "-ladvapi32", "-lbcrypt", "-lsynchronization"])
        .arg("-o")
        .arg(out);
    run(&mut c).map(|_| ())
}

// ---------------------------------------------------------------- end to end

fn translate(generator: &str, nxrs: &Path, exefs_parent: &Path, gen: &Path) -> R<()> {
    match generator {
        "rust" => run(Command::new(nxrs).arg("translate").arg(exefs_parent).arg("--out").arg(gen).args(["--jobs", "4"])).map(|_| ()),
        "mkw" => {
            let script = root().join("tools/mkw_gen.py");
            let py = std::env::var("NXRT_MKW_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|_| root().join("../../build/venv-mkw/Scripts/python.exe"));
            run(Command::new(py).env("PYTHONDONTWRITEBYTECODE", "1").arg("-B").arg("-I").arg(&script).arg("--exefs").arg(exefs_parent.join("exefs")).arg("--out").arg(gen).args(["--jobs", "4"]))
                .map(|_| ())
        }
        g => Err(format!("unknown generator {g}")),
    }
}

fn e2e(work: &Path, generator: &str, nxrs: &Path, tests: &[String]) -> R<bool> {
    build_guest(work, tests)?;
    let mut all = true;
    for t in tests {
        let dir = work.join(t);
        let gen = dir.join(format!("gen-{generator}"));
        let exe = dir.join(format!("{t}-{generator}.exe"));
        let step = translate(generator, nxrs, &dir, &gen).and_then(|_| link(&gen, &exe));
        if let Err(e) = step {
            println!("[{t}] BUILD FAILED: {e}");
            all = false;
            continue;
        }
        let out = Command::new(&exe).arg(dir.join("exefs")).output().map_err(|e| e.to_string())?;
        let got = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        let want = std::fs::read_to_string(root().join("guest/tests").join(format!("{t}.expected"))).unwrap().replace("\r\n", "\n");
        let ok = out.status.success() && got == want;
        all &= ok;
        println!("[{t}] {} (generator {generator}, exit {:?})", if ok { "PASS" } else { "FAIL" }, out.status.code());
        if !ok {
            println!("--- expected\n{want}--- got\n{got}--- stderr\n{}", String::from_utf8_lossy(&out.stderr));
        } else {
            for line in got.lines() {
                println!("    {line}");
            }
        }
    }
    Ok(all)
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let positional = |skip: &[&str]| -> Vec<String> {
        let mut v = vec![];
        let mut i = 1;
        while i < args.len() {
            if skip.contains(&args[i].as_str()) {
                i += 2;
                continue;
            }
            v.push(args[i].clone());
            i += 1;
        }
        v
    };
    let r = match args.first().map(String::as_str) {
        Some("guest") => {
            let out = PathBuf::from(arg(&args, "--out").unwrap_or("build/guest".into()));
            build_guest(&out, &tests_in(&root().join("guest/tests"))).map(|_| true)
        }
        Some("link") => {
            let (Some(g), Some(o)) = (arg(&args, "--gen"), arg(&args, "--out")) else {
                eprintln!("usage: nxrt-dev link --gen <dir> --out <exe>");
                std::process::exit(2);
            };
            link(Path::new(&g), Path::new(&o)).map(|_| true)
        }
        Some("e2e") => {
            let work = PathBuf::from(arg(&args, "--work").unwrap_or_else(|| root().join("../../build/runtime-e2e").display().to_string()));
            let generator = arg(&args, "--generator").unwrap_or("rust".into());
            let nxrs = PathBuf::from(arg(&args, "--nxrs").unwrap_or_else(|| {
                std::env::current_exe().unwrap().parent().unwrap().join("nxrs.exe").display().to_string()
            }));
            let mut tests = positional(&["--work", "--generator", "--nxrs"]);
            if tests.is_empty() {
                tests = tests_in(&root().join("guest/tests"));
            }
            e2e(&work, &generator, &nxrs, &tests)
        }
        _ => {
            eprintln!("usage: nxrt-dev guest|link|e2e (see the source header)");
            std::process::exit(2);
        }
    };
    match r {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("nxrt-dev: {e}");
            std::process::exit(1);
        }
    }
}

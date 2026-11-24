//! The build step: the generated C compiled with clang at the measured game-code flag set, one object per chunk kept under a
//! content key (a changed chunk recompiles alone), then linked with the runtime (`nxrt.lib`, statically) into
//! `<name>_code.dll`, and the small loader `<name>.exe` that starts it.
//!
//! Why the runtime is linked statically into the code DLL, not built as `nxrt.dll`:
//! - the generated code calls `rt_dispatch` on every indirect branch and call outside its chunk; inside one image
//!   that is a direct call, across DLLs an import-table call;
//! - the code and the runtime it was built against can never be mismatched (no ABI check at load needed);
//! - the dispatch table and the fault handler see one PE image (the table's 32-bit slots need all lifted code within
//!   4 GiB, which one image guarantees);
//! - a runtime update relinks only: every chunk object is reused (seconds).
//!
//! The executable carries no runtime at all: it loads `<its name>_code.dll` from its folder and calls the DLL's one
//! export, `nxrs_game_main`, so the same small file serves every game.
//!
//! Linux (x86-64 and AArch64): the same split as `<name>` (an ELF loader, no extension) and `<name>_code.so`, built with clang for
//! `TARGET_LINUX` and linked by ld.lld: the objects position-independent (`-fPIC`), the shared object
//! linked `-Bsymbolic` (the generated code's calls into the runtime bind inside the image, no PLT on `rt_dispatch`)
//! and `--no-undefined`, with the engine's `libnxfront.a` / `libnxrt.a` beside nxrs-build. The loader raises its
//! stack limit to 16 MB and runs itself again when the limit was lower (the main thread's stack is sized at exec).

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// The target every object is built for with Visual Studio's libraries: the engine is an MSVC-ABI static library.
pub const TARGET: &str = "x86_64-pc-windows-msvc";
/// On a machine without Visual Studio: the bundled llvm-mingw and the engine built for `x86_64-pc-windows-gnullvm`
/// (`gnu\libnxfront.a`, `gnu\libnxrt.a`), linked against the UCRT that every Windows 10 has.
pub const TARGET_GNU: &str = "x86_64-w64-windows-gnu";
/// Linux: the system's C library, the engine built for the host (`libnxfront.a`, `libnxrt.a`); x86-64 or AArch64.
pub const TARGET_LINUX: &str = if cfg!(target_arch = "aarch64") {
    "aarch64-unknown-linux-gnu"
} else {
    "x86_64-unknown-linux-gnu"
};

/// The game code's processor baseline: x86-64-v3 (AVX2, FMA, BMI2); AArch64 Linux: ARMv8.2-A with FP16, RCpc,
/// dot product and CRC32 (mkw-recomp's AArch64 baseline: Cortex-A76 / Neoverse N1 and later, Apple M1).
pub const MARCH: &str = if cfg!(target_arch = "aarch64") {
    "-march=armv8.2-a+fp16+rcpc+dotprod+crc"
} else {
    "-march=x86-64-v3"
};

/// The game-code flags: -O1 without the vectorizers, frame pointers or debug info (measured: fewer instructions than
/// either compiler's defaults), for `MARCH`. `-fms-runtime-lib=dll` matches the CRT the Rust
/// runtime links.
pub const GEN_FLAGS: &[&str] = &[
    "-std=gnu11",
    "-O1",
    "-fno-strict-aliasing",
    "-Wno-return-type",
    "-Wno-unused-label",
    "-fvisibility=hidden",
    MARCH,
    "-fno-vectorize",
    "-fno-slp-vectorize",
    "-fomit-frame-pointer",
    "-g0",
    "-fms-runtime-lib=dll",
    "-w",
];

/// Flags for the loader executable (ordinary C, size over speed).
const STUB_FLAGS: &[&str] = &["-std=gnu11", "-O2", "-fms-runtime-lib=dll", "-w"];

/// System libraries the Rust runtime needs (`cargo rustc -p runtime -- --print native-static-libs`), plus what the
/// loader uses. `NXRS_LINK_LIBS` (space-separated) adds more when a runtime crate starts needing them.
const SYS_LIBS: &[&str] = &[
    "-lkernel32",
    "-lntdll",
    "-luserenv",
    "-lws2_32",
    "-ldbghelp",
    "-ladvapi32",
    "-lbcrypt",
    "-lsynchronization",
    "-luser32",
];

#[derive(Debug, Clone)]
pub struct Clang {
    pub path: PathBuf,
    /// "clang version 21.1.8"
    pub version: String,
    /// What an object depends on of the compiler: `--version` without InstalledDir (a moved toolchain keeps its
    /// objects), and the target.
    pub id: String,
    /// llvm-mingw and the gnullvm engine (no Visual Studio), else clang with Visual Studio's libraries.
    pub gnu: bool,
}

impl Clang {
    pub fn target(&self) -> &'static str {
        if cfg!(target_os = "linux") {
            TARGET_LINUX
        } else if self.gnu {
            TARGET_GNU
        } else {
            TARGET
        }
    }

    /// The game-code flags for this toolchain (the MSVC CRT choice only applies to the MSVC target; Linux objects go
    /// into a shared object: position-independent).
    pub fn gen_flags(&self) -> Vec<String> {
        let linux = cfg!(target_os = "linux");
        let mut v: Vec<String> = GEN_FLAGS
            .iter()
            .filter(|f| !((self.gnu || linux) && f.starts_with("-fms-runtime-lib")))
            .map(|s| s.to_string())
            .collect();
        if linux {
            v.push("-fPIC".into());
        }
        v
    }
}

/// Whether Visual Studio's C++ build tools (the MSVC libraries the MSVC-ABI engine links) are installed: vswhere's
/// answer, or a developer prompt's `VCToolsInstallDir`.
pub fn have_visual_studio() -> bool {
    if cfg!(not(windows)) {
        return false;
    }
    if std::env::var_os("VCToolsInstallDir").is_some() {
        return true;
    }
    let vswhere =
        PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .is_ok_and(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
}

/// The bundled llvm-mingw's clang: `NXRS_LLVM_MINGW\bin`, beside nxrs (`llvm-mingw\bin`, `..\toolchain\llvm-mingw\bin`),
/// or a development checkout's `build\tools\llvm-mingw`.
pub fn llvm_mingw_clang() -> Option<PathBuf> {
    let mut v = vec![];
    if let Some(d) = std::env::var_os("NXRS_LLVM_MINGW") {
        v.push(Path::new(&d).join(r"bin\clang.exe"));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        v.push(dir.join(r"llvm-mingw\bin\clang.exe"));
        v.push(dir.join(r"..\toolchain\llvm-mingw\bin\clang.exe"));
        for up in dir.ancestors().take(5) {
            v.push(up.join(r"build\tools\llvm-mingw\bin\clang.exe"));
        }
    }
    v.into_iter().find(|p| p.is_file())
}

/// Which toolchain: `NXRS_TOOLCHAIN=msvc|gnu`, else MSVC when it can work here (Visual Studio installed, a clang
/// found, the MSVC engine library beside nxrs-build or `NXRS_ENGINE_LIB`), else the bundled llvm-mingw when it is
/// there. A release may ship only the gnullvm engine: then a machine with Visual Studio builds with llvm-mingw too.
pub fn use_gnu() -> bool {
    if cfg!(target_os = "linux") {
        return false; // one toolchain on Linux: the system's clang and lld
    }
    match std::env::var("NXRS_TOOLCHAIN")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("gnu") | Some("mingw") | Some("gnullvm") => true,
        Some("msvc") => false,
        _ => llvm_mingw_clang().is_some() && !msvc_ready(),
    }
}

/// Everything the MSVC toolchain needs: Visual Studio's libraries, a clang, and the MSVC engine library.
fn msvc_ready() -> bool {
    let lib = std::env::var_os("NXRS_ENGINE_LIB")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join("nxfront.lib")))
        });
    lib.is_some_and(|l| l.is_file())
        && candidates(None).iter().any(|p| p.is_file())
        && have_visual_studio()
}

/// The folder of the gnullvm engine libraries: `NXRS_GNU_LIBS`, else `gnu\` beside nxrs.
pub fn gnu_lib_dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("NXRS_GNU_LIBS") {
        return Ok(PathBuf::from(d));
    }
    Ok(std::env::current_exe()?.parent().unwrap().join("gnu"))
}

fn version_of(path: &Path) -> Result<(String, String)> {
    let out = Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("starting {}", path.display()))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let first = text.lines().next().unwrap_or("").to_string();
    let Some(at) = first.find("clang version ") else {
        bail!("{} is not clang: {first:?}", path.display())
    };
    let version = first[at..]
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    let id = text
        .lines()
        .filter(|l| !l.starts_with("InstalledDir:"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok((version, id))
}

fn which(exe: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(exe))
            .find(|p| p.is_file())
    })
}

/// Where clang is looked for, in order: `NXRS_CLANG`; `--cc` when it names a clang; the bundled one beside nxrs
/// (`clang\bin`, `..\toolchain\clang\bin`); `NXRT_LLVM` (the runtime's own tools use it); LLVM's installer's folder;
/// PATH.
pub fn candidates(cc_hint: Option<&str>) -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        return candidates_linux(cc_hint);
    }
    let mut v = vec![];
    if let Some(p) = std::env::var_os("NXRS_CLANG") {
        v.push(PathBuf::from(p));
    }
    if let Some(cc) = cc_hint {
        let name = Path::new(cc)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.contains("clang") {
            v.push(PathBuf::from(cc));
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        v.push(dir.join(r"clang\bin\clang.exe"));
        v.push(dir.join(r"..\toolchain\clang\bin\clang.exe"));
    }
    if let Some(d) = std::env::var_os("NXRT_LLVM") {
        v.push(Path::new(&d).join("clang.exe"));
    }
    v.push(PathBuf::from(r"C:\Program Files\LLVM\bin\clang.exe"));
    if let Some(p) = which("clang.exe") {
        v.push(p);
    }
    v
}

/// Linux: `NXRS_CLANG`; `--cc` when it names a clang; the bundled one beside nxrs-build (`clang/bin/clang`,
/// `../toolchain/clang/bin/clang`); `NXRT_LLVM`; PATH's `clang`, then its versioned names (`clang-21` .. `clang-15`).
fn candidates_linux(cc_hint: Option<&str>) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(p) = std::env::var_os("NXRS_CLANG") {
        v.push(PathBuf::from(p));
    }
    if let Some(cc) = cc_hint {
        if Path::new(cc)
            .file_name()
            .is_some_and(|n| n.to_string_lossy().contains("clang"))
        {
            v.push(PathBuf::from(cc));
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        v.push(dir.join("clang/bin/clang"));
        v.push(dir.join("../toolchain/clang/bin/clang"));
    }
    if let Some(d) = std::env::var_os("NXRT_LLVM") {
        v.push(Path::new(&d).join("clang"));
    }
    for n in
        std::iter::once("clang".to_string()).chain((15..=21).rev().map(|k| format!("clang-{k}")))
    {
        if let Some(p) = which(&n) {
            v.push(p);
        }
    }
    v
}

/// The linker clang drives on Linux (`-fuse-ld=lld`): ld.lld beside that clang or on PATH.
pub fn find_lld(clang: &Clang) -> Option<PathBuf> {
    let beside = clang
        .path
        .parent()
        .map(|d| d.join("ld.lld"))
        .filter(|p| p.is_file());
    beside
        .or_else(|| which("ld.lld"))
        .or_else(|| (15..=21).rev().find_map(|k| which(&format!("ld.lld-{k}"))))
}

pub fn find_clang(cc_hint: Option<&str>) -> Result<Clang> {
    if use_gnu() {
        let path = llvm_mingw_clang().ok_or_else(|| anyhow!("NXRS_TOOLCHAIN=gnu, but llvm-mingw is not bundled (llvm-mingw\\bin beside nxrs, or NXRS_LLVM_MINGW)"))?;
        let (version, id) = version_of(&path)?;
        return Ok(Clang {
            path,
            version,
            id: format!("{id}\ntarget {TARGET_GNU}"),
            gnu: true,
        });
    }
    let tried = candidates(cc_hint);
    for path in &tried {
        if path.is_file() {
            let (version, id) = version_of(path)?;
            return Ok(Clang {
                path: path.clone(),
                version,
                id: format!("{id}\ntarget {TARGET}"),
                gnu: false,
            });
        }
    }
    bail!(
        "clang not found (tried NXRS_CLANG, --cc, the bundled clang, NXRT_LLVM, LLVM's folder, PATH): {}",
        tried.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("; ")
    )
}

/// What the code DLL links: the whole engine (`nxfront.lib`: front end, os services, graphics and the runtime;
/// the game enters `nxfront_main`) for a title built on Nintendo's SDK, or the runtime alone (`nxrt.lib`,
/// `nxrt_main`) for homebrew that brings its own (the runtime's tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engine {
    pub lib: PathBuf,
    /// The front end (`nxfront_main`, the os layer's cpu.h overlay) or the runtime alone (`nxrt_main`).
    pub front: bool,
}

impl Engine {
    pub fn entry(&self) -> &'static str {
        if self.front {
            "nxfront_main"
        } else {
            "nxrt_main"
        }
    }
}

/// `nxfront.lib` beside nxrs (or `NXRS_ENGINE_LIB`; `gnu\libnxfront.a` for llvm-mingw) for an SDK title, else the
/// runtime (`find_runtime_lib`).
pub fn find_engine(sdk_title: bool, gnu: bool) -> Result<Engine> {
    if !sdk_title {
        return Ok(Engine {
            lib: find_runtime_lib(gnu)?,
            front: false,
        });
    }
    if let Some(p) = std::env::var_os("NXRS_ENGINE_LIB") {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(Engine {
                lib: p,
                front: true,
            })
        } else {
            Err(anyhow!("NXRS_ENGINE_LIB={} does not exist", p.display()))
        };
    }
    let here = std::env::current_exe()?.parent().unwrap().to_path_buf();
    let p = if cfg!(target_os = "linux") {
        here.join("libnxfront.a")
    } else if gnu {
        gnu_lib_dir()?.join("libnxfront.a")
    } else {
        here.join("nxfront.lib")
    };
    if p.is_file() {
        Ok(Engine {
            lib: p,
            front: true,
        })
    } else {
        bail!("the engine library {} is missing (cargo build --release -p front, or set NXRS_ENGINE_LIB)", p.display())
    }
}

/// System libraries the whole engine needs on top of [`SYS_LIBS`]: COM, the window and its theme, the shell, audio
/// (WinMM), HID controllers (SetupAPI, HID), video recording (Media Foundation).
const ENGINE_LIBS: &[&str] = &[
    "-lole32",
    "-loleaut32",
    "-lgdi32",
    "-lshell32",
    "-ldwmapi",
    "-luxtheme",
    "-limm32",
    "-lcomctl32",
    "-lwinmm",
    "-lpropsys",
    "-llegacy_stdio_definitions",
    "-lsetupapi",
    "-lhid",
    "-lmfplat",
    "-lmfreadwrite",
    "-lmfuuid",
];

/// The runtime library the code DLL links: `NXRS_RUNTIME_LIB`, else `nxrt.lib` beside nxrs (`gnu\libnxrt.a`).
pub fn find_runtime_lib(gnu: bool) -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("NXRS_RUNTIME_LIB") {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(anyhow!("NXRS_RUNTIME_LIB={} does not exist", p.display()))
        };
    }
    let exe = std::env::current_exe()?;
    let p = if cfg!(target_os = "linux") {
        exe.parent().unwrap().join("libnxrt.a")
    } else if gnu {
        gnu_lib_dir()?.join("libnxrt.a")
    } else {
        exe.parent().unwrap().join("nxrt.lib")
    };
    if p.is_file() {
        Ok(p)
    } else {
        bail!("the runtime library {} is missing (cargo build --release -p runtime, or set NXRS_RUNTIME_LIB)", p.display())
    }
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// One compile unit.
#[derive(Debug, Clone)]
pub struct Unit {
    pub src: PathBuf,
    pub obj: PathBuf,
    pub flags: Vec<String>,
    /// Header bytes the unit can include, already hashed (see [`headers_key`]).
    pub headers: String,
}

/// sha256 over the compiler id and every header (name and bytes): what all objects of a build depend on besides
/// their own C and flags.
pub fn headers_key(clang: &Clang, headers: &[PathBuf]) -> Result<String> {
    let mut h = Sha256::new();
    h.update(clang.id.as_bytes());
    let mut hs: Vec<&PathBuf> = headers.iter().collect();
    hs.sort();
    for p in hs {
        h.update(b"\0");
        h.update(p.file_name().unwrap().to_string_lossy().as_bytes());
        h.update(b"\0");
        h.update(std::fs::read(p).with_context(|| format!("reading {}", p.display()))?);
    }
    Ok(hex(&h.finalize()))
}

/// An object's key: the headers' key, its flags with the build folder's path taken out (a moved build keeps its
/// objects), and its C.
pub fn object_key(unit: &Unit, root: &Path) -> Result<String> {
    let mut h = Sha256::new();
    h.update(unit.headers.as_bytes());
    let root = root.display().to_string();
    for f in &unit.flags {
        h.update(f.replace(&root, "<build>").as_bytes());
        h.update(b"\0");
    }
    h.update(std::fs::read(&unit.src).with_context(|| format!("reading {}", unit.src.display()))?);
    Ok(hex(&h.finalize()))
}

fn stamp_of(obj: &Path) -> PathBuf {
    obj.with_extension("key")
}

/// A crash of the compiler itself (as opposed to an error in the source). This machine has been seen to crash
/// compilers at random under parallel load; a crash is retried, an error is not.
fn crashed(code: Option<i32>, stderr: &str) -> bool {
    stderr.contains("PLEASE submit a bug report")
        || stderr.contains("FILES TO THE BUG REPORT")
        || matches!(code, Some(c) if (c as u32) >= 0xC000_0000)
        || code.is_none()
}

fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    // A file just written is often held for a moment by antivirus or the indexer: a rename then fails with
    // ERROR_SHARING_VIOLATION / ACCESS_DENIED.
    let mut last = None;
    for k in 0..40 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250 * (1 + k / 8)));
            }
        }
    }
    Err(last.unwrap())
}

fn compile_one(clang: &Clang, unit: &Unit, key: &str) -> Result<(), String> {
    let tmp = unit.obj.with_extension("o.part");
    let mut last = String::new();
    for attempt in 0..3 {
        let out = Command::new(&clang.path)
            .arg(format!("--target={}", clang.target()))
            .args(&unit.flags)
            .arg("-c")
            .arg(&unit.src)
            .arg("-o")
            .arg(&tmp)
            .output()
            .map_err(|e| format!("starting {}: {e}", clang.path.display()))?;
        if out.status.success() {
            rename_retry(&tmp, &unit.obj).map_err(|e| format!("{}: {e}", unit.obj.display()))?;
            std::fs::write(stamp_of(&unit.obj), key)
                .map_err(|e| format!("{}: {e}", unit.obj.display()))?;
            return Ok(());
        }
        let _ = std::fs::remove_file(&tmp);
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        last = format!(
            "{}: {}",
            unit.src.file_name().unwrap().to_string_lossy(),
            tail(&err, 3000)
        );
        if !crashed(out.status.code(), &err) {
            break;
        }
        eprintln!(
            "compile: {}: the compiler crashed (attempt {}), trying again",
            unit.src.display(),
            attempt + 1
        );
    }
    Err(last)
}

fn tail(s: &str, n: usize) -> &str {
    let s = s.trim();
    if s.len() <= n {
        return s;
    }
    let mut at = s.len() - n;
    while !s.is_char_boundary(at) {
        at += 1;
    }
    &s[at..]
}

#[derive(Debug, Default, Clone)]
pub struct CompileReport {
    pub units: usize,
    pub compiled: usize,
    pub seconds: f64,
}

/// Compiles every unit whose object is missing or stale, `jobs` at a time, the largest first; `say` gets
/// "compiled n/m (Ns)" every ten units and at the end.
pub fn compile(
    clang: &Clang,
    units: &[Unit],
    root: &Path,
    jobs: usize,
    say: &(dyn Fn(&str) + Sync),
) -> Result<CompileReport> {
    let t0 = Instant::now();
    let mut order: Vec<(u64, usize)> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (std::fs::metadata(&u.src).map(|m| m.len()).unwrap_or(0), i))
        .collect();
    order.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let compiled = AtomicUsize::new(0);
    let error: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(_, ui)) = order.get(i) else { break };
                if error.lock().unwrap().is_some() {
                    break;
                }
                let unit = &units[ui];
                let r = (|| -> Result<(), String> {
                    let key = object_key(unit, root).map_err(|e| e.to_string())?;
                    let fresh = unit.obj.is_file()
                        && std::fs::read_to_string(stamp_of(&unit.obj)).ok().as_deref()
                            == Some(key.as_str());
                    if fresh {
                        return Ok(());
                    }
                    let _ = std::fs::remove_file(stamp_of(&unit.obj));
                    compiled.fetch_add(1, Ordering::Relaxed);
                    compile_one(clang, unit, &key)
                })();
                if let Err(e) = r {
                    error.lock().unwrap().get_or_insert(e);
                    break;
                }
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 10 == 0 || n == units.len() {
                    say(&format!(
                        "  compiled {n}/{} ({:.0}s)",
                        units.len(),
                        t0.elapsed().as_secs_f64()
                    ));
                }
            });
        }
    });
    if let Some(e) = error.into_inner().unwrap() {
        bail!("Compilation failed:\n{e}");
    }
    Ok(CompileReport {
        units: units.len(),
        compiled: compiled.into_inner(),
        seconds: t0.elapsed().as_secs_f64(),
    })
}

/// Deletes objects (and their keys) in `dir` that no unit produces any more (chunks an earlier version had).
pub fn remove_stale_objects(dir: &Path, units: &[Unit]) -> usize {
    let keep: std::collections::HashSet<&Path> = units.iter().map(|u| u.obj.as_path()).collect();
    let mut n = 0;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "o") && !keep.contains(p.as_path()) {
            let _ = std::fs::remove_file(stamp_of(&p));
            if std::fs::remove_file(&p).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// Response-file text: one quoted path a line, forward slashes (lld-link and the clang driver read both).
pub fn response_file(objs: &[PathBuf]) -> String {
    objs.iter()
        .map(|o| format!("\"{}\"\n", o.display().to_string().replace('\\', "/")))
        .collect()
}

fn run_link(clang: &Clang, args: &[String], cwd: &Path) -> Result<()> {
    let out = Command::new(&clang.path)
        .arg(format!("--target={}", clang.target()))
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("starting {}", clang.path.display()))?;
    if !out.status.success() {
        bail!(
            "Linking failed:\n{}{}",
            tail(&String::from_utf8_lossy(&out.stdout), 2000),
            tail(&String::from_utf8_lossy(&out.stderr), 4000)
        );
    }
    Ok(())
}

/// What the gnullvm engine adds (`--print native-static-libs`): the windows crate's import library (shipped in
/// `gnu\`) and LLVM's unwinder, linked statically so the game needs no llvm-mingw DLL.
const GNU_LIBS: &[&str] = &[
    "-lwindows.0.52.0",
    "-Wl,-Bstatic",
    "-lunwind",
    "-Wl,-Bdynamic",
];

fn extra_libs(front: bool, gnu: bool) -> Vec<String> {
    let mut v: Vec<String> = SYS_LIBS.iter().map(|s| s.to_string()).collect();
    if front {
        // the MinGW CRT has the stdio functions MSVC keeps in legacy_stdio_definitions
        v.extend(
            ENGINE_LIBS
                .iter()
                .filter(|l| !(gnu && l.contains("legacy_stdio")))
                .map(|s| s.to_string()),
        );
    }
    if gnu {
        if let Ok(d) = gnu_lib_dir() {
            v.push(format!("-L{}", d.display()));
        }
        v.extend(GNU_LIBS.iter().map(|s| s.to_string()));
    }
    if let Ok(more) = std::env::var("NXRS_LINK_LIBS") {
        v.extend(more.split_whitespace().map(String::from));
    }
    v
}

/// `<dll>`: every object and the runtime library, through a response file (hundreds of objects pass the command
/// line's limit). Written as `<dll>.part` and renamed, so a failed link leaves the previous DLL whole.
/// Linux: what the engine's static libraries need of the system (`cargo rustc --crate-type staticlib --
/// --print native-static-libs`): the runtime's, and for the whole engine ALSA (cpal's sound), the codec libraries
/// (opus, libvpx, dav1d) and FFmpeg's libavcodec / libavutil (H.264 and AAC).
const LINUX_SYS_LIBS: &[&str] = &[
    "-lgcc_s",
    "-lutil",
    "-lrt",
    "-lpthread",
    "-lm",
    "-ldl",
    "-lc",
];
const LINUX_ENGINE_LIBS: &[&str] = &[
    "-lopus",
    "-lvpx",
    "-ldav1d",
    "-lavcodec",
    "-lavutil",
    "-lasound",
];

/// `<name>_code.so` on Linux: every object and the engine library, `-Bsymbolic`, nothing left undefined.
fn link_so(
    clang: &Clang,
    objs: &[PathBuf],
    engine: &Engine,
    obj_dir: &Path,
    so: &Path,
) -> Result<()> {
    let rsp = obj_dir.join("link.rsp");
    std::fs::write(&rsp, response_file(objs))?;
    let part = so.with_file_name(format!(
        "{}.part",
        so.file_name().unwrap().to_string_lossy()
    ));
    let mut args: Vec<String> = vec![
        "-shared".into(),
        "-fuse-ld=lld".into(),
        "-o".into(),
        part.display().to_string(),
        format!("@{}", rsp.display()),
        engine.lib.display().to_string(),
        "-Wl,-Bsymbolic".into(),
        "-Wl,--no-undefined".into(),
        "-Wl,-z,noexecstack".into(),
        format!("-Wl,-soname,{}", so.file_name().unwrap().to_string_lossy()),
    ];
    if engine.front {
        args.extend(LINUX_ENGINE_LIBS.iter().map(|s| s.to_string()));
    }
    args.extend(LINUX_SYS_LIBS.iter().map(|s| s.to_string()));
    if let Ok(more) = std::env::var("NXRS_LINK_LIBS") {
        args.extend(more.split_whitespace().map(String::from));
    }
    run_link(clang, &args, obj_dir)?;
    rename_retry(&part, so)
        .with_context(|| format!("{} is in use. Close the game and try again.", so.display()))?;
    Ok(())
}

pub fn link_dll(
    clang: &Clang,
    objs: &[PathBuf],
    engine: &Engine,
    obj_dir: &Path,
    dll: &Path,
) -> Result<()> {
    if cfg!(target_os = "linux") {
        return link_so(clang, objs, engine, obj_dir, dll);
    }
    let rsp = obj_dir.join("link.rsp");
    std::fs::write(&rsp, response_file(objs))?;
    let part = dll.with_file_name(format!(
        "{}.part",
        dll.file_name().unwrap().to_string_lossy()
    ));
    let mut args: Vec<String> = vec!["-shared".into(), "-fuse-ld=lld".into()];
    if !clang.gnu {
        args.push("-fms-runtime-lib=dll".into());
    }
    args.extend([
        "-o".into(),
        part.display().to_string(),
        format!("@{}", rsp.display()),
        engine.lib.display().to_string(),
        // the import library and export file stay with the objects, not beside the game
        if clang.gnu {
            format!("-Wl,--out-implib,{}", obj_dir.join("code.dll.a").display())
        } else {
            format!("-Wl,/implib:{}", obj_dir.join("code.lib").display())
        },
    ]);
    if clang.gnu {
        // compiler-rt's builtins, statically: no llvm-mingw DLL beside the game
        args.extend(["-static-libgcc".into(), "--rtlib=compiler-rt".into()]);
    }
    args.extend(extra_libs(engine.front, clang.gnu));
    run_link(clang, &args, obj_dir)?;
    rename_retry(&part, dll)
        .with_context(|| format!("{} is in use. Close the game and try again.", dll.display()))?;
    Ok(())
}

/// The loader: `<name>.exe` loads `<name>_code.dll` beside it and calls `nxrs_game_main(argc, argv)`. Its data
/// folder `<name>_data` is handed over as NXRS_DATA, and its `exefs` as NXRT_EXEFS (the runtime's module folder),
/// unless the environment sets them already.
pub const STUB_C: &str = r#"#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <wchar.h>

typedef int (*nxrs_entry)(int argc, char **argv);

/* Hybrid systems otherwise put this unknown executable on the integrated GPU. These exports are the documented
   NVIDIA / AMD application hints (read from the executable, not a DLL) and do nothing on systems without switchable
   graphics (Turbo 6aace78). */
__declspec(dllexport) DWORD NvOptimusEnablement = 1;
__declspec(dllexport) int AmdPowerXpressRequestHighPerformance = 1;

/* A windowed program: a failure before the game's own log exists goes to stderr when it is redirected, and to a message
   box for a player who started the game from the launcher or the Explorer. */
static void fail(const wchar_t *what, const wchar_t *path, DWORD err) {
    wchar_t msg[4200];
    _snwprintf(msg, 4200, L"%ls: %ls (error %lu)", what, path, (unsigned long)err);
    msg[4199] = 0;
    HANDLE e = GetStdHandle(STD_ERROR_HANDLE);
    if (e && e != INVALID_HANDLE_VALUE && GetFileType(e) != FILE_TYPE_UNKNOWN) fwprintf(stderr, L"%ls\n", msg);
    else MessageBoxW(NULL, msg, L"nxrs", MB_OK | MB_ICONERROR);
}

int main(int argc, char **argv) {
    static wchar_t self[32768], dll[32768], data[32768], exefs[32768];
    DWORD n = GetModuleFileNameW(NULL, self, 32768);
    if (n == 0 || n >= 32768) { fail(L"cannot read the executable's path", L"", GetLastError()); return 1; }
    size_t len = wcslen(self);
    if (len > 4 && _wcsicmp(self + len - 4, L".exe") == 0) self[len - 4] = 0;
    if (_snwprintf(dll, 32768, L"%ls_code.dll", self) < 0 || _snwprintf(data, 32768, L"%ls_data", self) < 0
        || _snwprintf(exefs, 32768, L"%ls_data\\exefs", self) < 0) { fail(L"path too long", self, 0); return 1; }
    if (!GetEnvironmentVariableW(L"NXRS_DATA", NULL, 0)) SetEnvironmentVariableW(L"NXRS_DATA", data);
    if (!GetEnvironmentVariableW(L"NXRT_EXEFS", NULL, 0)) SetEnvironmentVariableW(L"NXRT_EXEFS", exefs);
    HMODULE code = LoadLibraryExW(dll, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!code) { fail(L"cannot load the game code", dll, GetLastError()); return 1; }
    nxrs_entry entry = (nxrs_entry)(void *)GetProcAddress(code, "nxrs_game_main");
    if (!entry) { fail(L"the game code has no nxrs_game_main (rebuild the game)", dll, GetLastError()); return 1; }
    return entry(argc, argv);
}
"#;

/// The loader on Linux: `<name>` loads `<name>_code.so` beside it (dlopen) and calls `nxrs_game_main(argc, argv)`,
/// with `NXRS_DATA` / `NXRT_EXEFS` set as on Windows. The main thread's stack is 16 MB as there: a lower limit is raised
/// and the loader runs itself again (the main stack is sized when a program starts). A failure before the game's
/// own log exists goes to stderr and, without a terminal, to the desktop's message box (zenity / kdialog).
pub const STUB_C_LINUX: &str = r#"#define _GNU_SOURCE
#include <dlfcn.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <spawn.h>
#include <unistd.h>

extern char **environ;
typedef int (*nxrs_entry)(int argc, char **argv);

static void fail(const char *what, const char *path, const char *why) {
    char msg[8192];
    snprintf(msg, sizeof msg, "%s: %s (%s)", what, path, why ? why : "");
    fprintf(stderr, "%s\n", msg);
    if (!isatty(2) && (getenv("DISPLAY") || getenv("WAYLAND_DISPLAY"))) {
        char *z[] = {"zenity", "--error", "--no-markup", "--title=nxrs", "--text", msg, NULL};
        char *k[] = {"kdialog", "--title", "nxrs", "--error", msg, NULL};
        pid_t pid;
        if (!posix_spawnp(&pid, z[0], NULL, NULL, z, environ) || !posix_spawnp(&pid, k[0], NULL, NULL, k, environ)) {
            int st;
            waitpid(pid, &st, 0);
        }
    }
}

int main(int argc, char **argv) {
    static char self[PATH_MAX], lib[PATH_MAX + 16], data[PATH_MAX + 16], exefs[PATH_MAX + 32];
    ssize_t n = readlink("/proc/self/exe", self, sizeof self - 1);
    if (n <= 0) { fail("cannot read the executable's path", "/proc/self/exe", NULL); return 1; }
    self[n] = 0;
    /* the game's start-up runs on the main thread: 16 MB of stack, as the Windows loader links it */
    struct rlimit rl;
    const rlim_t want = 16u << 20;
    if (!getrlimit(RLIMIT_STACK, &rl) && rl.rlim_cur != RLIM_INFINITY && rl.rlim_cur < want && !getenv("NXRS_STACK_SET")) {
        rl.rlim_cur = (rl.rlim_max == RLIM_INFINITY || rl.rlim_max >= want) ? want : rl.rlim_max;
        if (!setrlimit(RLIMIT_STACK, &rl)) {
            setenv("NXRS_STACK_SET", "1", 1);
            execv(self, argv);
        }
    }
    unsetenv("NXRS_STACK_SET");
    snprintf(lib, sizeof lib, "%s_code.so", self);
    snprintf(data, sizeof data, "%s_data", self);
    snprintf(exefs, sizeof exefs, "%s_data/exefs", self);
    setenv("NXRS_DATA", data, 0);
    setenv("NXRT_EXEFS", exefs, 0);
    void *code = dlopen(lib, RTLD_NOW | RTLD_LOCAL);
    if (!code) { fail("cannot load the game code", lib, dlerror()); return 1; }
    nxrs_entry entry = (nxrs_entry)dlsym(code, "nxrs_game_main");
    if (!entry) { fail("the game code has no nxrs_game_main (rebuild the game)", lib, dlerror()); return 1; }
    return entry(argc, argv);
}
"#;

/// Builds the loader into `exe` (the object is cached under its key in `obj_dir`).
pub fn link_exe(clang: &Clang, obj_dir: &Path, exe: &Path) -> Result<()> {
    if cfg!(target_os = "linux") {
        let src = obj_dir.join("nxrs_loader.c");
        crate::write_if_changed(&src, STUB_C_LINUX.as_bytes())?;
        let unit = Unit {
            src,
            obj: obj_dir.join("nxrs_loader.o"),
            flags: vec!["-std=gnu11".into(), "-O2".into(), "-w".into()],
            headers: headers_key(clang, &[])?,
        };
        compile(clang, std::slice::from_ref(&unit), obj_dir, 1, &|_| {})?;
        let part = exe.with_file_name(format!(
            "{}.part",
            exe.file_name().unwrap().to_string_lossy()
        ));
        let args: Vec<String> = vec![
            "-fuse-ld=lld".into(),
            "-o".into(),
            part.display().to_string(),
            unit.obj.display().to_string(),
            "-ldl".into(),
        ];
        run_link(clang, &args, obj_dir)?;
        rename_retry(&part, exe).with_context(|| {
            format!("{} is in use. Close the game and try again.", exe.display())
        })?;
        return Ok(());
    }
    let src = obj_dir.join("nxrs_loader.c");
    crate::write_if_changed(&src, STUB_C.as_bytes())?;
    let unit = Unit {
        src,
        obj: obj_dir.join("nxrs_loader.o"),
        flags: STUB_FLAGS
            .iter()
            .filter(|f| !(clang.gnu && f.starts_with("-fms-runtime-lib")))
            .map(|s| s.to_string())
            .collect(),
        headers: headers_key(clang, &[])?,
    };
    compile(clang, std::slice::from_ref(&unit), obj_dir, 1, &|_| {})?;
    let part = exe.with_file_name(format!(
        "{}.part",
        exe.file_name().unwrap().to_string_lossy()
    ));
    let mut args: Vec<String> = vec![
        "-fuse-ld=lld".into(),
        "-o".into(),
        part.display().to_string(),
        unit.obj.display().to_string(),
        "-lkernel32".into(),
        "-luser32".into(),
    ];
    if clang.gnu {
        // a windowed program (mainCRTStartup calls main), the engine's start-up on a 16 MB main stack
        args.extend([
            "-mwindows".into(),
            "-static-libgcc".into(),
            "--rtlib=compiler-rt".into(),
            "-Wl,--stack,0x1000000".into(),
        ]);
    } else {
        args.extend([
            "-fms-runtime-lib=dll".into(),
            // a windowed program: no console opens beside the game (the front end writes <name>.log)
            "-Wl,/SUBSYSTEM:WINDOWS".into(),
            "-Wl,/ENTRY:mainCRTStartup".into(),
            // the engine's start-up runs on the process's main thread
            "-Wl,/STACK:0x1000000".into(),
        ]);
    }
    run_link(clang, &args, obj_dir)?;
    rename_retry(&part, exe)
        .with_context(|| format!("{} is in use. Close the game and try again.", exe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_clang() -> Clang {
        Clang {
            path: "clang".into(),
            version: "clang version 0".into(),
            id: "clang version 0\ntarget x".into(),
            gnu: false,
        }
    }

    #[test]
    fn object_key_follows_source_flags_and_headers_but_not_the_build_folder() {
        let d = std::env::temp_dir().join(format!("nxrs-cc-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        for sub in ["a", "b"] {
            std::fs::write(d.join(sub).join("x.c"), "int x;").unwrap();
            std::fs::write(d.join(sub).join("cpu.h"), "// h").unwrap();
        }
        let c = fake_clang();
        let unit = |sub: &str, flag: &str| Unit {
            src: d.join(sub).join("x.c"),
            obj: d.join(sub).join("x.o"),
            flags: vec![
                format!("-I{}", d.join(sub).join("include").display()),
                flag.into(),
            ],
            headers: headers_key(&c, &[d.join(sub).join("cpu.h")]).unwrap(),
        };
        let ka = object_key(&unit("a", "-O1"), &d.join("a")).unwrap();
        let kb = object_key(&unit("b", "-O1"), &d.join("b")).unwrap();
        assert_eq!(ka, kb, "the same C in another build folder keeps its key");
        assert_ne!(
            ka,
            object_key(&unit("a", "-O2"), &d.join("a")).unwrap(),
            "flags are in the key"
        );
        std::fs::write(d.join("a").join("cpu.h"), "// changed").unwrap();
        assert_ne!(
            ka,
            object_key(&unit("a", "-O1"), &d.join("a")).unwrap(),
            "headers are in the key"
        );
        std::fs::write(d.join("b").join("x.c"), "int y;").unwrap();
        assert_ne!(
            kb,
            object_key(&unit("b", "-O1"), &d.join("b")).unwrap(),
            "the C is in the key"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn crash_detection() {
        assert!(crashed(Some(0xC0000005u32 as i32), ""));
        assert!(crashed(Some(1), "PLEASE submit a bug report to ..."));
        assert!(!crashed(Some(1), "x.c:1:1: error: unknown type name"));
    }

    #[test]
    fn response_file_quotes_with_forward_slashes() {
        assert_eq!(
            response_file(&[PathBuf::from(r"C:\a b\c.o")]),
            "\"C:/a b/c.o\"\n"
        );
    }

    /// The names a PE image exports (its export directory).
    #[cfg(windows)]
    fn pe_exports(d: &[u8]) -> Vec<String> {
        let u16_at = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]) as usize;
        let u32_at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
        let pe = u32_at(0x3c);
        let (nsec, optsz, opt) = (u16_at(pe + 6), u16_at(pe + 20), pe + 24);
        let exp_rva = u32_at(opt + 112);
        let off = |rva: usize| {
            (0..nsec).find_map(|i| {
                let s = opt + optsz + 40 * i;
                let (vs, va, rs, rp) = (
                    u32_at(s + 8),
                    u32_at(s + 12),
                    u32_at(s + 16),
                    u32_at(s + 20),
                );
                (va <= rva && rva < va + vs.max(rs)).then(|| rva - va + rp)
            })
        };
        let Some(e) = (exp_rva != 0).then(|| off(exp_rva)).flatten() else {
            return vec![];
        };
        let (n, names) = (u32_at(e + 24), off(u32_at(e + 32)).unwrap());
        (0..n)
            .map(|i| {
                let o = off(u32_at(names + 4 * i)).unwrap();
                String::from_utf8_lossy(&d[o..o + d[o..].iter().position(|&c| c == 0).unwrap()])
                    .into_owned()
            })
            .collect()
    }

    /// Turbo 6aace78: the game's executable exports NvOptimusEnablement and AmdPowerXpressRequestHighPerformance, so a
    /// hybrid laptop runs it on the discrete GPU. Built with the clang a build would use (skipped without one).
    #[cfg(windows)]
    #[test]
    fn loader_exports_the_gpu_hints() {
        assert!(STUB_C.contains("__declspec(dllexport) DWORD NvOptimusEnablement = 1;"));
        assert!(
            STUB_C.contains("__declspec(dllexport) int AmdPowerXpressRequestHighPerformance = 1;")
        );
        let Ok(clang) = find_clang(None) else {
            eprintln!("no clang: the link is not checked");
            return;
        };
        let d = std::env::temp_dir().join(format!("nxrs-loader-exports-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("game.exe");
        link_exe(&clang, &d, &exe).unwrap();
        let names = pe_exports(&std::fs::read(&exe).unwrap());
        let _ = std::fs::remove_dir_all(&d);
        assert!(
            names.iter().any(|n| n == "NvOptimusEnablement")
                && names
                    .iter()
                    .any(|n| n == "AmdPowerXpressRequestHighPerformance"),
            "{names:?}"
        );
    }
}

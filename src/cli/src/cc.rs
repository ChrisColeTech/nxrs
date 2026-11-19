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

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// The target every object is built for: the Rust runtime is an MSVC-ABI static library.
pub const TARGET: &str = "x86_64-pc-windows-msvc";

/// The game-code flags: -O1 without the vectorizers, frame pointers or debug info (measured: fewer instructions than
/// either compiler's defaults), for x86-64-v3 (AVX2, FMA, BMI2). `-fms-runtime-lib=dll` matches the CRT the Rust
/// runtime links.
pub const GEN_FLAGS: &[&str] = &[
    "-std=gnu11",
    "-O1",
    "-fno-strict-aliasing",
    "-Wno-return-type",
    "-Wno-unused-label",
    "-fvisibility=hidden",
    "-march=x86-64-v3",
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
    "-lkernel32", "-lntdll", "-luserenv", "-lws2_32", "-ldbghelp", "-ladvapi32", "-lbcrypt", "-lsynchronization", "-luser32",
];

#[derive(Debug, Clone)]
pub struct Clang {
    pub path: PathBuf,
    /// "clang version 21.1.8"
    pub version: String,
    /// What an object depends on of the compiler: `--version` without InstalledDir (a moved toolchain keeps its
    /// objects), and the target.
    pub id: String,
}

fn version_of(path: &Path) -> Result<(String, String)> {
    let out = Command::new(path).arg("--version").output().with_context(|| format!("starting {}", path.display()))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let first = text.lines().next().unwrap_or("").to_string();
    let Some(at) = first.find("clang version ") else { bail!("{} is not clang: {first:?}", path.display()) };
    let version = first[at..].split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    let id = text.lines().filter(|l| !l.starts_with("InstalledDir:")).collect::<Vec<_>>().join("\n");
    Ok((version, id))
}

fn which(exe: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(exe)).find(|p| p.is_file()))
}

/// Where clang is looked for, in order: `NXRS_CLANG`; `--cc` when it names a clang; the bundled one beside nxrs
/// (`clang\bin`, `..\toolchain\clang\bin`); `NXRT_LLVM` (the runtime's own tools use it); LLVM's installer's folder;
/// PATH.
pub fn candidates(cc_hint: Option<&str>) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(p) = std::env::var_os("NXRS_CLANG") {
        v.push(PathBuf::from(p));
    }
    if let Some(cc) = cc_hint {
        let name = Path::new(cc).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if name.contains("clang") {
            v.push(PathBuf::from(cc));
        }
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
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

pub fn find_clang(cc_hint: Option<&str>) -> Result<Clang> {
    let tried = candidates(cc_hint);
    for path in &tried {
        if path.is_file() {
            let (version, id) = version_of(path)?;
            return Ok(Clang { path: path.clone(), version, id: format!("{id}\ntarget {TARGET}") });
        }
    }
    bail!(
        "clang not found (tried NXRS_CLANG, --cc, the bundled clang, NXRT_LLVM, LLVM's folder, PATH): {}",
        tried.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("; ")
    )
}

/// The runtime library the code DLL links: `NXRS_RUNTIME_LIB`, else `nxrt.lib` beside nxrs.
pub fn find_runtime_lib() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("NXRS_RUNTIME_LIB") {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err(anyhow!("NXRS_RUNTIME_LIB={} does not exist", p.display())) };
    }
    let exe = std::env::current_exe()?;
    let p = exe.parent().unwrap().join("nxrt.lib");
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
            .arg(format!("--target={TARGET}"))
            .args(&unit.flags)
            .arg("-c")
            .arg(&unit.src)
            .arg("-o")
            .arg(&tmp)
            .output()
            .map_err(|e| format!("starting {}: {e}", clang.path.display()))?;
        if out.status.success() {
            rename_retry(&tmp, &unit.obj).map_err(|e| format!("{}: {e}", unit.obj.display()))?;
            std::fs::write(stamp_of(&unit.obj), key).map_err(|e| format!("{}: {e}", unit.obj.display()))?;
            return Ok(());
        }
        let _ = std::fs::remove_file(&tmp);
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        last = format!("{}: {}", unit.src.file_name().unwrap().to_string_lossy(), tail(&err, 3000));
        if !crashed(out.status.code(), &err) {
            break;
        }
        eprintln!("compile: {}: the compiler crashed (attempt {}), trying again", unit.src.display(), attempt + 1);
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
pub fn compile(clang: &Clang, units: &[Unit], root: &Path, jobs: usize, say: &(dyn Fn(&str) + Sync)) -> Result<CompileReport> {
    let t0 = Instant::now();
    let mut order: Vec<(u64, usize)> =
        units.iter().enumerate().map(|(i, u)| (std::fs::metadata(&u.src).map(|m| m.len()).unwrap_or(0), i)).collect();
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
                    let fresh = unit.obj.is_file() && std::fs::read_to_string(stamp_of(&unit.obj)).ok().as_deref() == Some(key.as_str());
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
                    say(&format!("  compiled {n}/{} ({:.0}s)", units.len(), t0.elapsed().as_secs_f64()));
                }
            });
        }
    });
    if let Some(e) = error.into_inner().unwrap() {
        bail!("Compilation failed:\n{e}");
    }
    Ok(CompileReport { units: units.len(), compiled: compiled.into_inner(), seconds: t0.elapsed().as_secs_f64() })
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
    objs.iter().map(|o| format!("\"{}\"\n", o.display().to_string().replace('\\', "/"))).collect()
}

fn run_link(clang: &Clang, args: &[String], cwd: &Path) -> Result<()> {
    let out = Command::new(&clang.path)
        .arg(format!("--target={TARGET}"))
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("starting {}", clang.path.display()))?;
    if !out.status.success() {
        bail!("Linking failed:\n{}{}", tail(&String::from_utf8_lossy(&out.stdout), 2000), tail(&String::from_utf8_lossy(&out.stderr), 4000));
    }
    Ok(())
}

fn extra_libs() -> Vec<String> {
    let mut v: Vec<String> = SYS_LIBS.iter().map(|s| s.to_string()).collect();
    if let Ok(more) = std::env::var("NXRS_LINK_LIBS") {
        v.extend(more.split_whitespace().map(String::from));
    }
    v
}

/// `<dll>`: every object and the runtime library, through a response file (hundreds of objects pass the command
/// line's limit). Written as `<dll>.part` and renamed, so a failed link leaves the previous DLL whole.
pub fn link_dll(clang: &Clang, objs: &[PathBuf], runtime: &Path, obj_dir: &Path, dll: &Path) -> Result<()> {
    let rsp = obj_dir.join("link.rsp");
    std::fs::write(&rsp, response_file(objs))?;
    let part = dll.with_file_name(format!("{}.part", dll.file_name().unwrap().to_string_lossy()));
    let mut args: Vec<String> = vec![
        "-shared".into(),
        "-fuse-ld=lld".into(),
        "-fms-runtime-lib=dll".into(),
        "-o".into(),
        part.display().to_string(),
        format!("@{}", rsp.display()),
        runtime.display().to_string(),
        // the import library and export file stay with the objects, not beside the game
        format!("-Wl,/implib:{}", obj_dir.join("code.lib").display()),
    ];
    args.extend(extra_libs());
    run_link(clang, &args, obj_dir)?;
    rename_retry(&part, dll).with_context(|| format!("{} is in use. Close the game and try again.", dll.display()))?;
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

static void fail(const wchar_t *what, const wchar_t *path, DWORD err) {
    fwprintf(stderr, L"%ls: %ls (error %lu)\n", what, path, (unsigned long)err);
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

/// Builds the loader into `exe` (the object is cached under its key in `obj_dir`).
pub fn link_exe(clang: &Clang, obj_dir: &Path, exe: &Path) -> Result<()> {
    let src = obj_dir.join("nxrs_loader.c");
    crate::write_if_changed(&src, STUB_C.as_bytes())?;
    let unit = Unit {
        src,
        obj: obj_dir.join("nxrs_loader.o"),
        flags: STUB_FLAGS.iter().map(|s| s.to_string()).collect(),
        headers: headers_key(clang, &[])?,
    };
    compile(clang, std::slice::from_ref(&unit), obj_dir, 1, &|_| {})?;
    let part = exe.with_file_name(format!("{}.part", exe.file_name().unwrap().to_string_lossy()));
    let args: Vec<String> = vec![
        "-fuse-ld=lld".into(),
        "-fms-runtime-lib=dll".into(),
        "-o".into(),
        part.display().to_string(),
        unit.obj.display().to_string(),
        "-lkernel32".into(),
    ];
    run_link(clang, &args, obj_dir)?;
    rename_retry(&part, exe).with_context(|| format!("{} is in use. Close the game and try again.", exe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_clang() -> Clang {
        Clang { path: "clang".into(), version: "clang version 0".into(), id: "clang version 0\ntarget x".into() }
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
            flags: vec![format!("-I{}", d.join(sub).join("include").display()), flag.into()],
            headers: headers_key(&c, &[d.join(sub).join("cpu.h")]).unwrap(),
        };
        let ka = object_key(&unit("a", "-O1"), &d.join("a")).unwrap();
        let kb = object_key(&unit("b", "-O1"), &d.join("b")).unwrap();
        assert_eq!(ka, kb, "the same C in another build folder keeps its key");
        assert_ne!(ka, object_key(&unit("a", "-O2"), &d.join("a")).unwrap(), "flags are in the key");
        std::fs::write(d.join("a").join("cpu.h"), "// changed").unwrap();
        assert_ne!(ka, object_key(&unit("a", "-O1"), &d.join("a")).unwrap(), "headers are in the key");
        std::fs::write(d.join("b").join("x.c"), "int y;").unwrap();
        assert_ne!(kb, object_key(&unit("b", "-O1"), &d.join("b")).unwrap(), "the C is in the key");
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
        assert_eq!(response_file(&[PathBuf::from(r"C:\a b\c.o")]), "\"C:/a b/c.o\"\n");
    }
}

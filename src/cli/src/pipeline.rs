//! `nxrs-build build`: package → data folder → C → shaders → code DLL + loader, in this output layout:
//!
//! ```text
//! <outdir>\<name>.exe            the loader (cc::STUB_C)
//! <outdir>\<name>_code.dll       the game's code, compiled, with the runtime linked in
//! <outdir>\<name>_data\          exefs\, romfs\ (the game's data, extracted) + romfs.source, control.nacp, the
//!                                icon; DLC (extracted too) and saves later
//! <outdir>\<name>.ini            written once, when missing
//! <outdir>\<name>_build[_v<N>|_base]\gen   the generated C (kept: a rebuild regenerates only what changed)
//!                                     \obj   one object per chunk under its content key, link.rsp
//! ```
//!
//! `<outdir>` defaults to `<the input's folder>\nxrs\<name>`, the folder the launcher builds into.

use crate::cc::{self, Clang, Unit};
use crate::out::Out;
use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use translator::aot::emit::EmitOptions;
use translator::aot::{generate, GenOptions};
use translator::package::{self, Applied, OpenOptions, UpdateChoice};

#[derive(Debug, Clone, Default)]
pub struct BuildArgs {
    pub input: PathBuf,
    pub name: Option<String>,
    pub outdir: Option<PathBuf>,
    pub jobs: usize,
    pub update: UpdateChoice,
    pub keys: Option<PathBuf>,
    /// `--cc`: used only when it names a clang
    pub cc: Option<String>,
    /// generate the C and stage the data folder, compile nothing
    pub no_build: bool,
    /// `--chunk`: about this many ARM instructions per C file (the legacy default was 30000); None: the translator's sizing
    pub chunk: Option<u64>,
    /// `--native`: compile for this processor (-march=native) instead of the baseline (`cc::MARCH`)
    pub native: bool,
    pub no_shaders: bool,
    /// v1: the DLC hint names `--install-dlc` (what a v1 caller passes); v2: `nxrs-build dlc install`
    pub v1: bool,
    /// v1: the legacy options that were passed and mean nothing here, said after the banner
    pub ignored: Option<String>,
    /// `--interp-only` (diagnostics): translate no function; the runtime interprets the whole game
    pub interp_only: bool,
    /// `--target web`: a browser build (docs/35): emcc instead of clang, `game.js` + `game.wasm` instead of the
    /// executable and code DLL, into `<input folder>\nxrs-web\<name>` by default
    pub web: bool,
    /// `--shaders-from <data folder>`: another build's translated shaders imported (`shaderimport`)
    pub shaders_from: Option<PathBuf>,
}

/// What the build produced.
pub struct Built {
    pub exe: PathBuf,
    pub data: PathBuf,
    pub timings: Vec<(&'static str, f64)>,
}

/// The name of the executable, the code DLL and the data folder: ASCII letters, digits, '-', '_' and spaces, an
/// accented letter without its accent (NFKD, then the ASCII part), "Game" when nothing is left. The launcher names
/// builds the same way (library.cpp outputNameFor); the runtime and the tools pass these paths as bytes.
pub fn output_name(title: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let s: String = title
        .nfkd()
        .filter(|c| c.is_ascii() && (c.is_ascii_alphanumeric() || "-_ ".contains(*c)))
        .collect();
    let s = s.trim();
    if s.is_empty() {
        "Game".into()
    } else {
        s.into()
    }
}

/// The NPDM's program name (META +0x20, 16 bytes), unless it is the SDK's default "Application".
pub fn npdm_name(npdm: &[u8]) -> Option<String> {
    if npdm.len() < 0x30 || &npdm[..4] != b"META" {
        return None;
    }
    let raw = &npdm[0x20..0x30];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let s = String::from_utf8_lossy(&raw[..end]).trim().to_string();
    (!s.is_empty() && s != "Application").then_some(s)
}

/// A folder keeps its whole name ("Game v1.0" keeps ".0"), a file loses its extension.
fn source_name(input: &Path) -> String {
    let base = if input.is_file() {
        input.file_stem()
    } else {
        input.file_name()
    };
    base.map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Game".into())
}

/// The folder beside the input that holds its builds: `<the input's folder>\nxrs\<name>` (owner, 2026-11-26).
pub const BUILDS_FOLDER: &str = "nxrs";

pub fn default_outdir(input: &Path, name: &str) -> PathBuf {
    let abs = std::path::absolute(input).unwrap_or_else(|_| input.to_path_buf());
    abs.parent()
        .unwrap_or(Path::new("."))
        .join(BUILDS_FOLDER)
        .join(name)
}

/// The folder beside the input that holds its web builds (`--target web`, docs/35): `<the input's folder>\nxrs-web\<name>`.
pub const WEB_BUILDS_FOLDER: &str = "nxrs-web";

pub fn default_web_outdir(input: &Path, name: &str) -> PathBuf {
    let abs = std::path::absolute(input).unwrap_or_else(|_| input.to_path_buf());
    abs.parent()
        .unwrap_or(Path::new("."))
        .join(WEB_BUILDS_FOLDER)
        .join(name)
}

/// The web build's entry (`int (int argc, char **argv, const RtModule *mods, int count)`), the web front's
/// `nxfront_main`; defined by the runtime once it is linked into the module (docs/35 W1/W2).
pub const WEB_ENTRY: &str = "nxweb_main";

/// A running game holds its executable and DLL open: say so before minutes of work, not after.
fn check_writable(paths: &[&Path]) -> Result<()> {
    for p in paths {
        if p.exists() && std::fs::OpenOptions::new().append(true).open(p).is_err() {
            bail!("{} is in use. Close the game and try again.", p.display());
        }
    }
    Ok(())
}

fn ini_text(title: &str) -> String {
    format!(
        "; {title} - settings\n; The launcher writes this game's settings here (renderer, display, controls); the game reads them at start.\n[Display]\nlanguage=1\n"
    )
}

/// Everything translation reads: the translator itself (this executable), the ExeFS, the patch table and the
/// options. While it matches, the generated C on disk is current and translation is skipped.
fn gen_key(exefs: &BTreeMap<String, Vec<u8>>, patches: &str, opts: &str) -> Result<String> {
    let mut h = Sha256::new();
    let exe = std::env::current_exe()?;
    h.update(std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?);
    for (name, data) in exefs {
        h.update(name.as_bytes());
        h.update((data.len() as u64).to_le_bytes());
        h.update(data);
    }
    h.update(patches.as_bytes());
    h.update(opts.as_bytes());
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

struct ModLine {
    name: String,
    base: u64,
    text: usize,
    build_id: String,
}

fn module_lines(exefs: &BTreeMap<String, Vec<u8>>, first_base: u64) -> Result<Vec<ModLine>> {
    let mut nsos = BTreeMap::new();
    let names = translator::loader::layout::order_with(exefs.keys());
    for name in &names {
        if let Some(raw) = exefs.get(name) {
            nsos.insert(
                name.clone(),
                translator::loader::Nso::parse(raw).map_err(|e| anyhow!("{name}: {e}"))?,
            );
        }
    }
    let lay = translator::loader::layout::layout_names_at(
        &names,
        &|n: &str| nsos.get(n).map(|x| x.size),
        first_base,
    )
    .map_err(|e| anyhow!(e))?;
    Ok(lay
        .into_iter()
        .map(|m| {
            let n = &nsos[&m.name];
            ModLine {
                name: m.name.clone(),
                base: m.base,
                text: n.text.1,
                build_id: n.build_id.clone(),
            }
        })
        .collect())
}

/// Registered NROs no loose RomFS file holds: from the build's cache (`<build>\nro\<SHA-256>.nro`), else inside the
/// RomFS's archives (data.arc's index), else found by a scan of the RomFS's large files (once; then cached).
fn nros_beyond_files(
    romfs: &package::romfs::RomFs,
    missing: &[[u8; 32]],
    cache: &Path,
    captured: &Path,
    out: &Out,
) -> std::result::Result<Vec<(String, Vec<u8>)>, package::PackageError> {
    let hex = |h: &[u8; 32]| h.iter().map(|b| format!("{b:02x}")).collect::<String>();
    // the cache of earlier builds, and the images runs of the game captured (os ro: an NRO it decoded at run time)
    let mut got: Vec<(String, Vec<u8>)> = missing
        .iter()
        .filter_map(|h| {
            [cache, captured]
                .iter()
                .find_map(|d| std::fs::read(d.join(format!("{}.nro", hex(h)))).ok())
                .map(|d| (format!("cache {}", hex(h)), d))
        })
        .collect();
    if got.len() == missing.len() {
        return Ok(got);
    }
    let have = |got: &Vec<(String, Vec<u8>)>| -> Vec<[u8; 32]> {
        got.iter().map(|(_, d)| Sha256::digest(d).into()).collect()
    };
    let left: Vec<[u8; 32]> = {
        let h = have(&got);
        missing.iter().filter(|m| !h.contains(m)).copied().collect()
    };
    got.extend(archive_nros(romfs, &left));
    let left: Vec<[u8; 32]> = {
        let h = have(&got);
        missing.iter().filter(|m| !h.contains(m)).copied().collect()
    };
    if !left.is_empty() {
        out.say(&format!("NRO modules: scanning the RomFS for {} registered NROs no file holds (once; cached in {})", left.len(), cache.display()));
        got.extend(package::nro::scan(romfs, &left, &|l| {
            out.say(&format!("  {l}"))
        })?);
    }
    let _ = std::fs::create_dir_all(cache);
    for (_, d) in &got {
        let h: [u8; 32] = Sha256::digest(d).into();
        let p = cache.join(format!("{}.nro", hex(&h)));
        if !p.exists() {
            let _ = std::fs::write(&p, d);
        }
    }
    Ok(got)
}

/// NROs inside the RomFS's archives (Super Smash Bros. Ultimate's data.arc: its fighters' code). Members whose
/// inflated head is an NRO header, read through the archive's index; the registered ones (`want`: SHA-256s).
fn archive_nros(romfs: &package::romfs::RomFs, want: &[[u8; 32]]) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    for (p, size) in romfs.walk() {
        if size < 0x38
            || romfs.read(&p, 0, 8).ok().as_deref() != Some(&shaders::formats::smash::MAGIC[..])
        {
            continue;
        }
        let read = |o: u64, n: u64| romfs.read(&p, o, n).map_err(|e| e.to_string());
        let Ok(files) = shaders::formats::smash::files(&read, &p) else {
            continue;
        };
        for e in &files {
            if e.3 < 0x100 || e.3 > package::nro::MAX_NRO {
                continue;
            }
            let Ok(d) = read(e.1, e.2.min(0x4000)) else {
                continue;
            };
            let head = if d.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
                package::nro::zstd_head(&d, 0x20)
            } else {
                Some(d)
            };
            if head.as_deref().and_then(|h| h.get(0x10..0x14)) != Some(b"NRO0") {
                continue;
            }
            if let Ok(full) = shaders::formats::smash::read_file(&read, e, &p) {
                let n = full
                    .get(0x18..0x1c)
                    .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                    .min(full.len());
                let h: [u8; 32] = Sha256::digest(&full[..n]).into();
                if want.contains(&h) {
                    out.push((format!("{p}#{:010x}", e.0), full[..n].to_vec()));
                    if out.len() >= want.len() {
                        return out;
                    }
                }
            }
        }
    }
    out
}

static CAPTURE_BOOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long the capture boot waits for the game to load the NROs it decodes (nxnative NRO_CAPTURE_SECS: Mario Tennis
/// Fever decodes its 24.7 MB NRO in under a second).
const CAPTURE_SECS: u64 = 120;

/// Boots the built game with `NXRS_NRO_CAPTURE` (os ro: the image of a LoadModule with no built copy is written to
/// `dir\<sha256>.nro` and the run ends) until an NRO of `missing` is there, the game exits or the time is up.
/// Returns how many of `missing` were captured.
fn capture_boot(exe: &Path, dir: &Path, missing: &[[u8; 32]], out: &Out) -> Result<usize> {
    let hex = |h: &[u8; 32]| h.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let have = || {
        missing
            .iter()
            .filter(|h| dir.join(format!("{}.nro", hex(h))).is_file())
            .count()
    };
    std::fs::create_dir_all(dir)?;
    out.say(&format!(
        "NRO modules: {} registered NRO(s) in no file; booting the game once to capture what it decodes at run time (up to {CAPTURE_SECS} s)",
        missing.len()
    ));
    let mut child = std::process::Command::new(exe)
        .env("NXRS_NRO_CAPTURE", std::path::absolute(dir)?)
        .env("NXRS_NO_ACTIVATE", "1")
        .env("NXRS_KEEP_FOCUS", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let t0 = Instant::now();
    while have() == 0 && t0.elapsed().as_secs() < CAPTURE_SECS {
        if child.try_wait()?.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if child.try_wait()?.is_none() {
        let _ = child.kill(); // this child only, by its handle
        let _ = child.wait();
    }
    let n = have();
    if n == 0 {
        out.say("NRO modules: the capture boot loaded none of them; the game runs without them (their loads fail)");
    }
    Ok(n)
}

/// The modules layout.json lists, as (name, base, size) strings.
fn layout_modules(gen: &Path) -> Result<Vec<(String, String, String)>> {
    let text = std::fs::read_to_string(gen.join("layout.json"))
        .with_context(|| format!("{}", gen.join("layout.json").display()))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let mut out = vec![];
    for m in v
        .as_array()
        .ok_or_else(|| anyhow!("layout.json: not a list"))?
    {
        let f = |k: &str| {
            m.get(k)
                .and_then(|x| x.as_str())
                .map(String::from)
                .ok_or_else(|| anyhow!("layout.json: no {k}"))
        };
        out.push((f("name")?, f("base")?, f("size")?));
    }
    Ok(out)
}

/// modules.c: the module table for the engine and the DLL's entry point, which calls `entry` (`nxfront_main` or
/// `nxrt_main`, both `int (int argc, char **argv, const RtModule *mods, int count)`).
pub fn modules_c(mods: &[(String, String, String)], entry: &str, web: bool) -> String {
    let mut s = String::from(
        "// Written by nxrs: the translated modules for nxrt_main, and the entry the loader calls.\n#include <stdlib.h>\n#include \"nxrt.h\"\n#include \"patches.h\"\n\n\
         volatile int g_fnprof __attribute__((visibility(\"hidden\")));\n\
         uint8_t rt_patch_on[256] __attribute__((visibility(\"hidden\")));\n\
         int nxfront_main(int argc, char **argv, const RtModule *mods, int count);\n\
         int nxweb_main(int argc, char **argv, const RtModule *mods, int count);\n\n\
         // A patch set is on by its default; its environment switch set to 0 turns it off, to anything else on.\n\
         static void nxrs_patch_sets(void) {\n\
         #define NXRS_SET(i, name, env, def) { const char *v = getenv(env); rt_patch_on[i] = (v && v[0]) ? (v[0] != '0') : (def); }\n\
         \x20   RT_PATCH_SETS(NXRS_SET)\n\
         #undef NXRS_SET\n\
         }\n\n",
    );
    if web {
        // the mod markers find the executable's folder through the OS; a web build has neither (its data, mods
        // included, is packaged by docs/35 W3/W5)
        s.push_str(
            "#include <stdio.h>\n#include <string.h>\nstatic void nxrs_mod_markers(void) {}\n\n",
        );
    } else {
        s.push_str(crate::mods::C_DECLS);
        s.push_str(&crate::mods::c_code());
    }
    for (n, _, _) in mods {
        s.push_str(&format!(
            "extern const uint64_t {n}_fn_addrs[];\nextern const GuestFn {n}_fn_ptrs[];\nextern const uint64_t {n}_fn_count;\n"
        ));
    }
    // the code library's one export: dllexport on Windows, default visibility in the Linux shared object, a kept
    // default-visibility symbol in the web module (emcc exports it by name)
    let export = if web {
        "__attribute__((used, visibility(\"default\")))"
    } else if cfg!(windows) {
        "__declspec(dllexport)"
    } else {
        "__attribute__((visibility(\"default\")))"
    };
    // the sets' switch names by index: with the switch table they go to the runtime, so the host can flip a set while
    // the game runs (a code cheat's toggle)
    s.push_str("\nstatic const char *const nxrs_patch_envs[] = {\n#define NXRS_ENV(i, name, env, def) [i] = env,\n    RT_PATCH_SETS(NXRS_ENV)\n#undef NXRS_ENV\n    0};\n");
    s.push_str(&format!(
        "\n{export} int nxrs_game_main(int argc, char **argv) {{\n    nxrs_patch_sets();\n    {{ const char *fp = getenv(\"NXRS_FNPROF\"); g_fnprof = fp && fp[0]; }}\n    nxrs_mod_markers();\n    nxrt_patch_table(rt_patch_on, nxrs_patch_envs, RT_PATCH_COUNT);\n    RtModule mods[] = {{\n"
    ));
    for (n, b, z) in mods {
        s.push_str(&format!(
            "        {{\"{n}\", {b}ull, {z}ull, {n}_fn_addrs, {n}_fn_ptrs, 0}},\n"
        ));
    }
    s.push_str("    };\n");
    for (i, (n, _, _)) in mods.iter().enumerate() {
        s.push_str(&format!("    mods[{i}].count = {n}_fn_count;\n"));
    }
    s.push_str(&format!(
        "    return {entry}(argc, argv, mods, {});\n}}\n",
        mods.len()
    ));
    s
}

/// The runtime's C interface (src/runtime/include/nxrt.h), carried by nxrs so a release needs only the libraries.
const NXRT_H: &str = include_str!("../../runtime/include/nxrt.h");

/// The compile units of a generated tree: every chunk and table of the modules in layout.json, and modules.c.
pub fn units(
    clang: &Clang,
    gen: &Path,
    obj: &Path,
    mods: &[(String, String, String)],
    entry: &str,
) -> Result<Vec<Unit>> {
    let include = gen.join("include");
    let mut headers: Vec<PathBuf> = std::fs::read_dir(&include)
        .with_context(|| format!("{}", include.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "h"))
        .collect();
    for h in ["layout.h", "patches.h"] {
        if gen.join(h).is_file() {
            headers.push(gen.join(h));
        }
    }
    let hk = cc::headers_key(clang, &headers)?;
    let gen_flags: Vec<String> = clang
        .gen_flags()
        .into_iter()
        .chain([
            format!("-I{}", include.display()),
            format!("-I{}", gen.display()),
        ])
        .collect();
    let mut v = vec![];
    for (m, _, _) in mods {
        let dir = gen.join(m);
        let mut srcs: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("{}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "c")
                    && !p.file_name().unwrap().to_string_lossy().starts_with('.')
            })
            .collect();
        srcs.sort();
        for src in srcs {
            let stem = src.file_stem().unwrap().to_string_lossy().into_owned();
            v.push(Unit {
                obj: obj.join(format!("{stem}.o")),
                src,
                flags: gen_flags.clone(),
                headers: hk.clone(),
            });
        }
    }
    crate::write_if_changed(&obj.join("nxrt.h"), NXRT_H.as_bytes())?;
    crate::write_if_changed(
        &obj.join("modules.c"),
        modules_c(mods, entry, clang.web.is_some()).as_bytes(),
    )?;
    let mut h2 = headers.clone();
    h2.push(obj.join("nxrt.h"));
    v.push(Unit {
        src: obj.join("modules.c"),
        obj: obj.join("modules.o"),
        flags: clang
            .gen_flags()
            .into_iter()
            .chain([
                format!("-I{}", obj.display()),
                format!("-I{}", include.display()),
                format!("-I{}", gen.display()),
            ])
            .collect(),
        headers: cc::headers_key(clang, &h2)?,
    });
    Ok(v)
}

/// The shader step: every shader the game ships, found and translated to SPIR-V into `<data>\shaders\`
/// (`shaders::prebuild_data`: the ExeFS, the RomFS the marker names and the add-on content installed there), and the
/// profile's shader tables (nxnative's prewarm seed, pairs and translation answers) into `<data>\shaders\profile\`.
/// A rebuild reuses every translated shader whose inputs and tools are unchanged. It runs beside the compile
/// (at most 4 translators so the compile keeps the CPU) and is never fatal:
/// a failure is a warning, the game's live translator covers what the build did not translate.
fn shaders_step(
    out: &Out,
    data: &Path,
    profile: Option<&str>,
    exact_profile_build: bool,
    a: &BuildArgs,
) {
    if a.no_shaders {
        out.say("shaders: not translated (--no-shaders); a game built so draws only the shaders an earlier build left in its data folder");
        return;
    }
    out.progress("shaders", 0, 0);
    if let Err(e) = shader_tables(out, data, profile, exact_profile_build) {
        out.say(&format!("shaders: the profile's tables not written ({e}); the game runs without its prewarm seed"));
    }
    // the profile's shader_key_skip: the keys the build writes are the ones the game's graphics computes
    shaders::nvn::set_key_skip(
        profile
            .and_then(profiles::get)
            .and_then(|p| p.settings.shader_key_skip),
    );
    let opts = shaders::Options {
        keys: a.keys.clone(),
        jobs: a.jobs.clamp(1, 4),
        ..shaders::Options::default()
    };
    // the prebuild's own lines give the real counts: `@progress shaders <done> <found>` as they are translated
    let counts = std::sync::Mutex::new(ShaderCounts::default());
    let say = |m: &str| {
        out.say(&format!("  {m}"));
        if let Some((d, n)) = counts.lock().unwrap().line(m) {
            out.progress("shaders", d, n);
        }
    };
    out.say("Translating the shaders (beside the compile)...");
    match shaders::prebuild_data(data, &opts, &say) {
        Ok(rep) => {
            match promote_live_shaders(&data.join("shaders")) {
                Ok(n) if n > 0 => out.say(&format!(
                    "shaders: promoted {n} runtime-derived variants into the build library"
                )),
                Ok(_) => {}
                Err(e) => out.say(&format!(
                    "shaders: runtime-derived variants not promoted ({e})"
                )),
            }
            if let Some(from) = &a.shaders_from {
                match crate::shaderimport::import(from, data) {
                    Ok(r) => out.say(&format!(
                        "shaders: {} imported from {} ({} already here, {} pair rows)",
                        r.shaders,
                        from.display(),
                        r.present,
                        r.pairs
                    )),
                    Err(e) => out.say(&format!("shaders: none imported ({e:#})")),
                }
            }
            match reconcile_shader_pairs(data) {
                Ok(removed) if removed > 0 => out.say(&format!(
                    "shaders: removed {removed} profile pairs absent from this build's manifest"
                )),
                Ok(_) => {}
                Err(e) => out.say(&format!("shaders: profile pairs not reconciled ({e})")),
            }
            let done = rep.translated + rep.reused + rep.in_library;
            out.progress("shaders", done, rep.found);
            out.say(&format!(
                "shaders: {} found, {} translated, {} reused, {} failed ({:.1}s)",
                rep.found, rep.translated, rep.reused, rep.failed, rep.total_seconds
            ));
        }
        Err(e) => out.say(&format!(
            "shaders: not translated at build time ({e}); the game translates them while it runs"
        )),
    }
}

/// Runtime translation is a durable derivation cache for shader variants generated by an engine at run time (Unity
/// is the current case). A later build promotes complete records into the normal library instead of reporting the same
/// keys missing forever. Incomplete/failed live attempts have no SPIR-V plus reflection pair and are ignored.
fn promote_live_shaders(root: &Path) -> Result<usize> {
    let live = root.join("live");
    if !live.is_dir() {
        return Ok(0);
    }
    let out_spv = root.join("spv");
    std::fs::create_dir_all(&out_spv)?;
    let mut manifest = std::fs::read_to_string(root.join("manifest.tsv"))
        .unwrap_or_else(|_| "# nxshaders 1\nkey\tstage\n".into());
    let mut pairs = std::fs::read_to_string(root.join("pairs.tsv"))
        .unwrap_or_else(|_| "# nxshaders pairs 1\n".into());
    let mut keys: std::collections::HashSet<String> = manifest
        .lines()
        .filter_map(|l| l.split_once('\t').map(|x| x.0.to_string()))
        .collect();
    let mut pair_rows: std::collections::HashSet<String> =
        pairs.lines().map(str::to_string).collect();
    let mut promoted = 0;
    for entry in std::fs::read_dir(&live)? {
        let dir = entry?.path();
        let Some(key) = dir
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| n.len() == 16 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        else {
            continue;
        };
        let spv = dir.join("spv").join(format!("{key}.spv"));
        let json = dir.join("spv").join(format!("{key}.json"));
        if !spv.is_file() || !json.is_file() {
            continue;
        }
        std::fs::copy(&spv, out_spv.join(format!("{key}.spv")))?;
        std::fs::copy(&json, out_spv.join(format!("{key}.json")))?;
        if keys.insert(key.to_string()) {
            if let Ok(rows) = std::fs::read_to_string(dir.join("manifest.tsv")) {
                for row in rows.lines().filter(|l| l.starts_with(key)) {
                    manifest.push_str(row);
                    manifest.push('\n');
                }
            }
            promoted += 1;
        }
        if let Ok(rows) = std::fs::read_to_string(dir.join("pairs.tsv")) {
            for row in rows
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                if pair_rows.insert(row.to_string()) {
                    pairs.push_str(row);
                    pairs.push('\n');
                }
            }
        }
    }
    std::fs::write(root.join("manifest.tsv"), manifest)?;
    std::fs::write(root.join("pairs.tsv"), pairs)?;
    Ok(promoted)
}

/// Progress read from the shader prebuild's lines (`shaders: N found in ...`, `shaders: translating N ...`,
/// `shaders: translated D of T ...`): every shader found and not in this round's work is already done (reused or in
/// the library), so done = found - T + D.
#[derive(Default)]
struct ShaderCounts {
    found: usize,
}

impl ShaderCounts {
    fn line(&mut self, m: &str) -> Option<(usize, usize)> {
        let rest = m.strip_prefix("shaders: ")?;
        let num = |s: &str| {
            s.split_whitespace()
                .next()
                .and_then(|w| w.parse::<usize>().ok())
        };
        if let Some(n) = rest.split_once(" found in ").and_then(|(n, _)| num(n)) {
            self.found = n;
            return Some((0, n));
        }
        let t = rest
            .strip_prefix("translating ")
            .and_then(num)
            .map(|t| (0, t));
        let t = t.or_else(|| {
            let (d, r) = rest.strip_prefix("translated ")?.split_once(" of ")?;
            Some((num(d)?, num(r)?))
        })?;
        let total = self.found.max(t.1);
        Some((total - t.1 + t.0, total))
    }
}

fn shader_table_allowed(name: &str, exact_profile_build: bool) -> bool {
    exact_profile_build || name.starts_with("shader_facts")
}

fn reconciled_pairs(manifest: &str, pairs: &str) -> (String, usize) {
    let keys: std::collections::HashSet<&str> = manifest
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split('\t').next())
        .filter(|key| key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    let mut removed = 0;
    let mut out = String::new();
    for line in pairs.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if !line.starts_with('#')
            && fields.len() >= 2
            && !(keys.contains(fields[0]) && keys.contains(fields[1]))
        {
            removed += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, removed)
}

fn reconcile_shader_pairs(data: &Path) -> Result<usize> {
    let shaders = data.join("shaders");
    let pairs_path = shaders.join("profile").join("shader_pairs.tsv");
    if !pairs_path.is_file() {
        return Ok(0);
    }
    let manifest = std::fs::read_to_string(shaders.join("manifest.tsv"))?;
    let pairs = std::fs::read_to_string(&pairs_path)?;
    let (filtered, removed) = reconciled_pairs(&manifest, &pairs);
    if removed > 0 {
        crate::write_if_changed(&pairs_path, filtered.as_bytes())?;
    }
    Ok(removed)
}

fn shader_tables(
    out: &Out,
    data: &Path,
    profile: Option<&str>,
    exact_profile_build: bool,
) -> Result<()> {
    let Some(id) = profile else { return Ok(()) };
    let prof_dir = data.join("shaders").join("profile");
    let tables: Vec<_> = profiles::shader_tables(id)
        .map_err(|e| anyhow!(e))?
        .into_iter()
        .filter(|(name, _)| shader_table_allowed(name, exact_profile_build))
        .collect();
    if !exact_profile_build {
        for name in ["pso_manifest.tsv", "pso_pairs.tsv", "shader_pairs.tsv"] {
            match std::fs::remove_file(prof_dir.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    if !tables.is_empty() {
        std::fs::create_dir_all(&prof_dir)?;
        for (name, text) in &tables {
            crate::write_if_changed(&prof_dir.join(name), text.as_bytes())?;
        }
        let names: Vec<&str> = tables.iter().map(|(n, _)| n.as_str()).collect();
        out.say(&format!(
            "shaders: the profile's tables ({}) in {}",
            names.join(", "),
            prof_dir.display()
        ));
    }
    Ok(())
}

/// `NXRS_PROBE=<module>:<offset>[,...]` (nxnative's DM_PROBE syntax, hex offsets): the patch table's `probe` lines.
pub fn probe_lines(spec: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    for p in spec
        .split([',', ' '])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let (m, o) = p
            .split_once(':')
            .ok_or_else(|| anyhow!("NXRS_PROBE: {p:?} is not <module>:<offset>"))?;
        let off = u64::from_str_radix(o.trim_start_matches("0x"), 16)
            .map_err(|_| anyhow!("NXRS_PROBE: {p:?}: the offset is not hex"))?;
        if off % 4 != 0 {
            bail!("NXRS_PROBE: {p:?}: not an instruction (4-byte aligned)");
        }
        out.push(format!("probe {m} {off:#x}"));
    }
    Ok(out)
}
/// `NXRS_CFLAGS`: extra compiler flags for the game code, split on white space (e.g. `-march`
/// for another machine's processor). They join the object keys, so changing them recompiles.
pub fn extra_cflags(env: Option<&str>) -> Vec<String> {
    env.map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default()
}

/// `--jobs` when none is given: the processor count, 4 if it cannot be read.
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

pub fn build(a: &BuildArgs, out: &Out) -> Result<Built> {
    let t_all = Instant::now();
    let mut timings = vec![];
    if !a.input.exists() {
        bail!("Cannot load the game: {} does not exist", a.input.display());
    }
    // the compiler and the runtime are checked first, not after minutes of translation
    let clang = if a.no_build {
        None
    } else if a.web {
        Some(cc::find_emcc()?)
    } else {
        Some(cc::find_clang(a.cc.as_deref())?)
    };
    out.say("nxrs - use your own copy of the game. No games or keys included.");
    if let Some(i) = &a.ignored {
        out.say(i);
    }
    out.progress("open", 0, 0);
    let game = package::open_game(
        &a.input,
        &OpenOptions {
            keys: a.keys.clone(),
            update: a.update.clone(),
        },
    )
    .map_err(|e| anyhow!("Cannot load the game: {e}"))?;
    if game.exefs.files.is_empty() {
        bail!(
            "Cannot load the game: {} has no ExeFS modules",
            a.input.display()
        );
    }
    // a title on Nintendo's SDK runs on the whole engine; homebrew brings its own and gets the runtime alone
    let engine = match &clang {
        // the web build links no engine library yet: its runtime joins the module in docs/35 W1
        Some(c) if c.web.is_some() => None,
        // a retail title (Nintendo's rtld and an NPDM beside its sdk); the runtime's own homebrew has neither
        Some(c) => Some(cc::find_engine(
            game.exefs.files.contains_key("sdk")
                && (game.exefs.files.contains_key("rtld")
                    || game.exefs.files.contains_key("main.npdm")),
            c.gnu,
        )?),
        None => None,
    };
    let first_base = if a.web {
        translator::loader::layout::WEB_FIRST_BASE
    } else {
        translator::loader::layout::FIRST_BASE
    };
    for m in module_lines(&game.exefs.files, first_base)? {
        out.say(&format!(
            "  {:<8} {:010X}  code {:.1} MB  build id {}",
            m.name,
            m.base,
            m.text as f64 / 1048576.0,
            m.build_id
        ));
    }
    let builds = crate::profile::module_builds(&game.exefs.files);
    let prof = crate::profile::find(game.exefs.title_id, &builds);
    out.say(&crate::profile::line(&prof, game.exefs.title_id, &builds));
    let patch_text = crate::profile::patch_table(&builds, out)?;
    let mut stub_facts: Option<String> = None;
    let patch_text =
        if std::env::var("NXRS_NO_PROFILE_PATCHES").is_ok_and(|v| !v.is_empty() && v != "0") {
            patch_text
        } else {
            let (sites, said) = crate::profile::match_sites(&prof, &game.exefs.files, &builds);
            for l in &said {
                out.say(&format!("patch matched: {l}"));
            }
            let text = crate::profile::with_match_sites(patch_text, &sites);
            let st = crate::profile::stubs(&prof, &game.exefs.files, &builds);
            for l in &st.said {
                out.say(&format!("patch stub: {l}"));
            }
            stub_facts = Some(st.facts());
            let (fence, said) = crate::profile::trinity_sites(&game.exefs.files, &builds);
            for l in &said {
                out.say(&format!("patch: {l}"));
            }
            crate::profile::with_lines(crate::profile::with_lines(text, &st.lines), &fence)
        };
    // NXRS_PROBE=<module>:<offset>,...: probe points the translator places (rt_probe logs the registers there)
    let probes = probe_lines(&std::env::var("NXRS_PROBE").unwrap_or_default())?;
    for l in &probes {
        out.say(&format!("probe: {l}"));
    }
    let patch_text = crate::profile::with_lines(patch_text, &probes);
    let title = a
        .name
        .clone()
        .or_else(|| game.exefs.files.get("main.npdm").and_then(|n| npdm_name(n)))
        .unwrap_or_else(|| source_name(&a.input));
    let name = output_name(&title);
    let web_title_id = game.exefs.title_id;
    let outdir = std::path::absolute(a.outdir.clone().unwrap_or_else(|| {
        if a.web {
            default_web_outdir(&a.input, &name)
        } else {
            default_outdir(&a.input, &name)
        }
    }))?;
    std::fs::create_dir_all(&outdir).with_context(|| format!("creating {}", outdir.display()))?;
    let base = outdir.join(&name);
    let with = |suffix: &str| PathBuf::from(format!("{}{suffix}", base.display()));
    // Linux: `<name>` and `<name>_code.so`; web: `game.js` and `game.wasm` (the folder is the game's)
    let (exe, dll, data) = if a.web {
        (
            outdir.join("game.js"),
            outdir.join("game.wasm"),
            with("_data"),
        )
    } else if cfg!(target_os = "linux") {
        (with(""), with("_code.so"), with("_data"))
    } else {
        (with(".exe"), with("_code.dll"), with("_data"))
    };
    // the player's mods: their code patches join the profile's table, their data bytes and RomFS go to the run
    let patch_text = crate::mods::prepare(&outdir, &game.exefs.files, &data, patch_text, out)?;
    // the code cheats of main's build: a patch set each, switched by the cheat's toggle at run time
    let patch_text = crate::cheats::prepare(&game.exefs.files, &data, patch_text, out)?;
    if !a.no_build {
        check_writable(&[&exe, &dll])?;
    }
    // another program (a separate update applied, or the one a package holds left out) gets a build folder of its
    // own, so switching back and forth compiles each once and afterwards only links
    let build_dir = match &game.applied {
        Applied::Separate(_) => with(&format!("_build_v{}", game.exefs.version.unwrap_or(0))),
        Applied::LeftOut => with("_build_base"),
        _ => with("_build"),
    };
    let (gen, obj) = (build_dir.join("gen"), build_dir.join("obj"));
    std::fs::create_dir_all(&obj)?;

    // NRO modules the game loads at run time: translated with the ExeFS at bases after it (package::nro)
    let nros = package::nro::collect_with(&game.romfs, &|missing| {
        nros_beyond_files(
            &game.romfs,
            missing,
            &build_dir.join("nro"),
            &data.join("nro"),
            out,
        )
    })
    .map_err(|e| anyhow!("Cannot read the game's NRO modules: {e}"))?;
    let mut code = game.exefs.files.clone();
    for n in &nros.nros {
        code.insert(n.name.clone(), n.bytes.clone());
    }
    if nros.registered > 0 {
        out.say(&format!(
            "NRO modules: {} of the {} the game's {} NRR files register, loaded by the game at run time{}{}",
            nros.nros.len(),
            nros.registered,
            nros.nrrs,
            if nros.refused.is_empty() { String::new() } else { format!("; {} online SDKs left out", nros.refused.len()) },
            if nros.missing.is_empty() { String::new() } else { format!("; {} not found in the RomFS", nros.missing.len()) },
        ));
    }

    // ---- translate
    let t = Instant::now();
    out.say("Analyzing the code...");
    out.progress("analyze", 0, 0);
    let patches = match &patch_text {
        Some(t) => {
            let p = build_dir.join("patches.txt");
            crate::write_if_changed(&p, t.as_bytes())?;
            Some(p)
        }
        None => None,
    };
    let mut opts = GenOptions {
        out: gen.clone(),
        jobs: a.jobs.max(1),
        emit: EmitOptions::default(),
        patches,
        first_base,
        ..GenOptions::default()
    };
    if let Some(n) = a.chunk.filter(|&n| n > 0) {
        // `--chunk` counts ARM instructions per C file; the translator sizes chunks by the C they make
        opts.chunk_bytes = n as i64 * translator::aot::discover::EST_INT;
        opts.chunk_fns = usize::MAX;
    }
    // execution hints: the code earlier runs reached that this build had not translated (the runtime's interpreter
    // wrote them to <data>\discovered.tsv); each becomes a function start when its module still has its words
    let hints_text =
        std::fs::read_to_string(data.join(translator::aot::hints::FILE)).unwrap_or_default();
    opts.hints = translator::aot::hints::parse(&hints_text);
    opts.interp_only = a.interp_only;
    let mut key_opts = format!(
        "{:?}{:?}",
        (opts.chunk_fns, opts.chunk_bytes, opts.big_fn),
        (opts.emit.reg_locals, opts.emit.callee_saved)
    );
    if !opts.hints.is_empty() {
        key_opts.push_str(&format!("|hints:{:?}", opts.hints));
    }
    if a.interp_only {
        key_opts.push_str("|interp-only");
    }
    let key = gen_key(&code, patch_text.as_deref().unwrap_or(""), &key_opts)?;
    let stamp = gen.join("nxrs.gen.key");
    let reuse = std::fs::read_to_string(&stamp).ok().as_deref() == Some(key.as_str())
        && gen.join("layout.json").is_file();
    if reuse {
        let mods = layout_modules(&gen)?;
        let chunks: usize = mods
            .iter()
            .map(|(m, _, _)| {
                std::fs::read_dir(gen.join(m))
                    .map(|d| {
                        d.flatten()
                            .filter(|e| e.file_name().to_string_lossy().ends_with(".c"))
                            .count()
                    })
                    .unwrap_or(0)
            })
            .sum();
        out.say("Translating ARM64 to C...");
        out.say(&format!(
            "  C generated: {chunks}/{chunks} chunks (unchanged since the last build)"
        ));
    } else {
        let _ = std::fs::remove_file(&stamp);
        let started = std::sync::atomic::AtomicBool::new(false);
        let t_gen = Instant::now();
        let report = generate(&code, &opts, &|line: &str| {
            if let Some(rest) = line.strip_prefix("C generated: ") {
                if !started.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    out.say("Translating ARM64 to C...");
                }
                out.say(&format!(
                    "  C generated: {rest} chunks ({:.0}s)",
                    t_gen.elapsed().as_secs_f64()
                ));
                if let Some((d, n)) = rest.split_once('/') {
                    out.progress(
                        "translate",
                        d.trim().parse().unwrap_or(0),
                        n.trim().parse().unwrap_or(0),
                    );
                }
            } else {
                out.say(&format!("  {line}"));
            }
        })?;
        translator::aot::runtime_headers::write(&gen)?;
        let fns: usize = report.modules.iter().map(|m| m.functions).sum();
        let bytes: usize = report.modules.iter().map(|m| m.c_bytes).sum();
        let files: usize = report.modules.iter().map(|m| m.chunks + 1).sum();
        out.say(&format!(
            "C code: {files} files, {:.1} MB, {fns} functions",
            bytes as f64 / 1e6
        ));
        if report.hints_used > 0 || report.hints_dropped > 0 {
            out.say(&format!(
                "Execution hints: {} function(s) earlier runs had to interpret are translated now{}",
                report.hints_used,
                if report.hints_dropped > 0 { format!("; {} left out (another version of the program)", report.hints_dropped) } else { String::new() }
            ));
        }
        if a.interp_only {
            out.say("--interp-only: no function translated; the game runs entirely in the interpreter (diagnostics)");
        }
        std::fs::write(&stamp, &key)?;
    }
    timings.push(("translate", t.elapsed().as_secs_f64()));

    // ---- data folder
    let t = Instant::now();
    out.progress("data", 0, 0);
    let v1 = a.v1;
    // the RomFS is written into the data folder: @progress in MB, and a RomFS: line every few seconds
    let said_at = std::cell::Cell::new(Instant::now());
    let progress = |done: u64, total: u64| {
        let mb = |b: u64| (b >> 20) as usize;
        out.progress("data", mb(done), mb(total));
        if said_at.get().elapsed().as_secs() >= 5 && done < total {
            out.say(&format!("RomFS: {} of {} MB written", mb(done), mb(total)));
            said_at.set(Instant::now());
        }
    };
    game.write_data_with(
        &data,
        &|m: &str| {
            // the hint names the option the caller's contract has
            if v1 {
                out.say(&m.replace(
                    "install it with nxrs dlc install (the launcher does)",
                    "install it with --install-dlc (the launcher does)",
                ));
            } else {
                out.say(&m.replace(
                    "install it with nxrs dlc install",
                    "install it with nxrs-build dlc install",
                ));
            }
        },
        &progress,
    )?;
    // the frame step's facts for the host (its state page writer); none: no frame-step patches
    let fs_txt = data.join("frame_step.txt");
    match stub_facts.as_deref().filter(|f| f.contains("frame_step 1")) {
        Some(f) => std::fs::write(&fs_txt, f).with_context(|| format!("{}", fs_txt.display()))?,
        None => {
            let _ = std::fs::remove_file(&fs_txt);
        }
    }
    // the engines' frame-rate places in main (Unity, Unreal, Monolith) for the host's frame lock
    let fl_txt = data.join("fps_lock.txt");
    let found = game
        .exefs
        .files
        .get("main")
        .and_then(|r| translator::loader::nso::Nso::parse(r).ok())
        .map(|n| crate::fpsfind::find_all(&n));
    match found {
        Some((lines, said)) if !lines.is_empty() || !said.is_empty() => {
            for l in &said {
                out.say(&format!("fps lock: {l}"));
            }
            let text = format!("# Written by nxrs: engine frame-rate places in main (os fpslock writes fps_target there).
{}", lines.iter().map(|l| format!("{l}
")).collect::<String>());
            std::fs::write(&fl_txt, text).with_context(|| format!("{}", fl_txt.display()))?;
        }
        _ => {
            let _ = std::fs::remove_file(&fl_txt);
        }
    }
    let nro_tsv = data.join("nro.tsv");
    if nros.nros.is_empty() && nros.refused.is_empty() {
        let _ = std::fs::remove_file(&nro_tsv);
    } else {
        let lay: Vec<(String, u64, u64)> = layout_modules(&gen)?
            .into_iter()
            .map(|(n, b, z)| {
                let h = |v: &str| u64::from_str_radix(v.trim_start_matches("0x"), 16).unwrap_or(0);
                (n, h(&b), h(&z))
            })
            .collect();
        crate::write_if_changed(&nro_tsv, package::nro::tsv(&nros, &lay).as_bytes())?;
    }
    let ini = with(".ini");
    // the web build has no executable to read an ini; its settings come with the host (docs/35 W5)
    if !a.web && !ini.exists() {
        std::fs::write(&ini, ini_text(&name))?;
    }
    timings.push(("data", t.elapsed().as_secs_f64()));
    out.progress("data", 1, 1);
    if a.no_build {
        return Ok(Built { exe, data, timings });
    }
    let clang = clang.unwrap();
    let entry = engine.as_ref().map_or(WEB_ENTRY, |e| e.entry());

    // ---- shaders (on a thread beside the compile) and compile and link
    let profile_id = prof.as_ref().map(|m| m.profile.id.as_str());
    let exact_profile_build = prof.as_ref().is_some_and(|m| m.how == profiles::How::Build);
    let compile_link = |timings: &mut Vec<(&'static str, f64)>| -> Result<()> {
        let t = Instant::now();
        let mods = layout_modules(&gen)?;
        let mut units = units(&clang, &gen, &obj, &mods, entry)?;
        let extra = extra_cflags(std::env::var("NXRS_CFLAGS").ok().as_deref());
        if clang.web.is_some() {
            cc::warm_emcc(&clang, &obj)?;
        }
        for u in &mut units {
            for f in &mut u.flags {
                if a.native && f == crate::cc::MARCH {
                    *f = "-march=native".into();
                }
            }
            u.flags.extend(extra.iter().cloned());
        }
        let (cc_name, cc_version) = match &clang.web {
            Some(e) => (
                format!("emcc from {}", e.root.display()),
                clang.version.clone(),
            ),
            None => (
                clang
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                clang
                    .version
                    .trim_start_matches("clang version ")
                    .to_string(),
            ),
        };
        out.say(&format!(
            "Compiling with {} {} (-O1{}{}, {} at a time)...",
            cc_name,
            cc_version,
            if a.native { ", -march=native" } else { "" },
            if extra.is_empty() {
                String::new()
            } else {
                format!(", NXRS_CFLAGS {}", extra.join(" "))
            },
            a.jobs.max(1)
        ));
        let total = units.len();
        let rep = cc::compile(&clang, &units, &build_dir, a.jobs.max(1), &|line: &str| {
            out.say(line);
            if let Some((d, n)) = line
                .trim()
                .strip_prefix("compiled ")
                .and_then(|r| r.split_once(' '))
                .and_then(|(dn, _)| dn.split_once('/'))
            {
                out.progress(
                    "compile",
                    d.parse().unwrap_or(0),
                    n.parse().unwrap_or(total),
                );
            }
        })?;
        cc::remove_stale_objects(&obj, &units);
        out.say(&format!(
            "  compile: {} of {} units compiled, the rest reused ({:.1}s)",
            rep.compiled, rep.units, rep.seconds
        ));
        timings.push(("compile", t.elapsed().as_secs_f64()));
        let t = Instant::now();
        out.say(&format!(
            "Linking {} ...",
            dll.file_name().unwrap().to_string_lossy()
        ));
        out.progress("link", 0, 0);
        let objs: Vec<PathBuf> = units.iter().map(|u| u.obj.clone()).collect();
        match &engine {
            Some(engine) => {
                out.say(&format!(
                    "  with {} ({})",
                    engine.lib.file_name().unwrap().to_string_lossy(),
                    engine.entry()
                ));
                cc::link_dll(&clang, &objs, engine, &obj, &dll)?;
                cc::link_exe(&clang, &obj, &exe)?;
            }
            None => {
                // the hot/cold split (websplit.rs) when the data folder has a recorded hot set and NXRS_WEB_SPLIT=1:
                // opt-in: with pthreads every engine thread instantiates the hot module's ~290k placeholder imports
                // (~140 MiB each) and Chrome's renderer is killed at ~16.6 GB (docs/waves/WEB-W6b.md §1)
                let hot = crate::websplit::hot_set(&data)
                    .filter(|_| std::env::var("NXRS_WEB_SPLIT").as_deref() == Ok("1"));
                match cc::find_web_lib() {
                    Some(lib) => {
                        if let Some(root) = cc::web_lib_dev_tree(&lib) {
                            let secs = cc::refresh_web_lib(&root)?;
                            out.say(&format!(
                                "  web engine library brought up to date with {} ({secs:.1}s)",
                                root.display()
                            ));
                        }
                        cc::check_web_lib(&lib)?;
                        out.say(&format!("  with {} ({WEB_ENTRY})", lib.display()));
                        cc::link_web(&clang, &objs, Some(&lib), &obj, &exe, hot.is_some())?;
                    }
                    None => {
                        out.say("  no web engine library (scripts/build/web.ps1 -Crates os): the runtime's symbols are imports");
                        cc::link_web(&clang, &objs, None, &obj, &exe, hot.is_some())?;
                    }
                }
                let dir = exe.parent().unwrap_or(Path::new("."));
                match (&hot, &clang.web) {
                    (Some(h), Some(e)) => {
                        timings.push(("link", t.elapsed().as_secs_f64()));
                        let t = Instant::now();
                        for l in crate::websplit::split(
                            dir,
                            &e.root,
                            h,
                            translator::loader::layout::FIRST_BASE,
                            translator::loader::layout::WEB_FIRST_BASE,
                        )? {
                            out.say(&l);
                        }
                        timings.push(("split", t.elapsed().as_secs_f64()));
                        return Ok(());
                    }
                    (_, e) => {
                        crate::websplit::clean_unsplit(dir);
                        out.say(&format!(
                            "  one module (split: a {} in the data folder and NXRS_WEB_SPLIT=1)",
                            crate::websplit::HOTSET
                        ));
                        // identical functions folded (WEB-W11)
                        if let Some(e) = e.as_ref().filter(|_| crate::websplit::fold_on()) {
                            out.say(&crate::websplit::fold(dir, &e.root)?);
                        }
                    }
                }
            }
        }
        timings.push(("link", t.elapsed().as_secs_f64()));
        Ok(())
    };
    let (built, shader_secs) = std::thread::scope(|s| {
        let data = &data;
        let web = a.web;
        let sh = std::thread::Builder::new()
            .name("shader prebuild".into())
            .spawn_scoped(s, move || {
                let t = Instant::now();
                // the web build translates them too: its GPU turns each module the replay makes from them into WGSL
                // when it is created (src/graphics/src/web), the page serves them on first use (docs/35 W4)
                let _ = web;
                // never fatal, a panic of the prebuild thread included
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shaders_step(out, data, profile_id, exact_profile_build, a))).is_err() {
                    out.say("shaders: not translated at build time (the prebuild stopped); the game translates them while it runs");
                }
                t.elapsed().as_secs_f64()
            })
            .expect("shader prebuild thread");
        let r = compile_link(&mut timings);
        if r.is_ok() && !sh.is_finished() {
            out.say("The game is built; its shaders are still being translated...");
        }
        (r, sh.join().unwrap_or(0.0))
    });
    built?;
    if a.web {
        // the shared-font fallback is the engine's built-in OFL font on the web (os services/misc.rs `fallback_ttf`;
        // its license goes beside the page, webhost.rs). Builds before WEB-W6 copied this machine's system font
        // here, which may not be redistributed: removed
        let stale = data.join("fonts").join("fallback.ttf");
        if stale.is_file() {
            std::fs::remove_file(&stale)?;
            let _ = std::fs::remove_dir(data.join("fonts"));
            out.say("  removed fonts\\fallback.ttf (a system font an older web build copied; the engine has its own)");
        }
        // what the page streams and prefetches (docs/35 W3)
        out.say(&crate::webpack::write(&outdir, &data, &name)?);
        // the recorded run's shader modules as a few packs the page fetches whole (webshaders.rs, WEB-W6b)
        out.say(&crate::webshaders::write(&outdir, &data)?);
        // gzip copies of the modules and descriptions, sent with Content-Encoding (webzip.rs)
        let t = Instant::now();
        let mut zipped = vec![];
        for f in [
            "game.wasm",
            crate::websplit::COLD,
            "game.js",
            crate::webpack::MANIFEST,
        ] {
            match crate::webzip::gzip_file(&outdir.join(f))? {
                Some((raw, z)) => zipped.push(format!(
                    "{f} {:.1} -> {:.1} MiB",
                    raw as f64 / 1048576.0,
                    z as f64 / 1048576.0
                )),
                None => {
                    let _ = std::fs::remove_file(crate::webzip::gz_path(&outdir.join(f)));
                }
            }
        }
        out.say(&format!(
            "Web gzip copies: {} ({:.1}s)",
            zipped.join(", "),
            t.elapsed().as_secs_f64()
        ));
        // the page, its workers and build.json: the folder is complete for any COOP/COEP server (docs/35 W5)
        out.say(&crate::webhost::write(&outdir, &data, &name, web_title_id)?);
    }
    timings.push(("shaders", shader_secs));
    timings.push(("total", t_all.elapsed().as_secs_f64()));
    // NROs the game's NRRs register and no file holds: the game decodes them at run time. Boot it once to capture
    // them (nxnative's capture boot), then build again with them
    if !a.web
        && !nros.missing.is_empty()
        && std::env::var_os("NXRS_NO_CAPTURE_BOOT").is_none()
        && !CAPTURE_BOOTED.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        let got = capture_boot(&exe, &build_dir.join("nro"), &nros.missing, out)?;
        if got > 0 {
            out.say(&format!(
                "NRO modules: {got} captured; building again with them"
            ));
            return build(a, out);
        }
    }
    Ok(Built { exe, data, timings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names_are_ascii() {
        assert_eq!(
            output_name("Pokémon Legends: Arceus"),
            "Pokemon Legends Arceus"
        );
        assert_eq!(output_name("  ★★  "), "Game");
        assert_eq!(
            output_name("Mario Kart 8 Deluxe 1G1U1D"),
            "Mario Kart 8 Deluxe 1G1U1D"
        );
        assert_eq!(output_name("Zelda_x-y (v1.0)"), "Zelda_x-y v10");
    }

    #[test]
    fn npdm_names() {
        let mut n = vec![0u8; 0x80];
        n[..4].copy_from_slice(b"META");
        n[0x20..0x2b].copy_from_slice(b"Application");
        assert_eq!(npdm_name(&n), None);
        n[0x20..0x30].fill(0);
        n[0x20..0x24].copy_from_slice(b"Game");
        assert_eq!(npdm_name(&n).as_deref(), Some("Game"));
    }

    #[test]
    fn default_outdir_is_beside_the_input() {
        #[cfg(windows)]
        let (input, want) = (r"P:\Dumps\X\x.nsp", r"P:\Dumps\X\nxrs\X");
        #[cfg(not(windows))]
        let (input, want) = ("/dumps/X/x.nsp", "/dumps/X/nxrs/X");
        assert_eq!(default_outdir(Path::new(input), "X"), PathBuf::from(want));
    }

    /// `--jobs` defaults to the CPU count; NXRS_CFLAGS adds white-space separated flags.
    #[test]
    fn nxrs_cflags_and_default_jobs() {
        assert_eq!(
            extra_cflags(Some(" -march=znver4  -mtune=znver4\t")),
            vec!["-march=znver4", "-mtune=znver4"]
        );
        assert!(extra_cflags(None).is_empty() && extra_cflags(Some("  ")).is_empty());
        assert_eq!(
            default_jobs(),
            std::thread::available_parallelism().unwrap().get()
        );
    }

    /// 6d04b3b: the shader step reports real counts from the prebuild's own lines (found, then translated D of T
    /// with the reused ones counted done).
    #[test]
    fn shader_progress_from_prebuild_lines() {
        let mut c = ShaderCounts::default();
        assert_eq!(
            c.line("shaders: 11609 found in 13.0 s (3 files, 12 MB read, 0 problems)"),
            Some((0, 11609))
        );
        assert_eq!(
            c.line("shaders: translating 11608 (1 reused) with 4 processes"),
            Some((1, 11609))
        );
        assert_eq!(
            c.line("shaders: translated 500 of 11608 (40 a second)"),
            Some((501, 11609))
        );
        assert_eq!(
            c.line("shaders: translated 11608 of 11608 (44 a second)"),
            Some((11609, 11609))
        );
        assert_eq!(c.line("shaders: 11609 found, 11608 translated (1 reused), 0 failed, 0 skipped, 1 valid, 0 invalid; 263 s"), None);
        assert_eq!(c.line("shaders: validated 3 of 9"), None);
    }

    #[test]
    fn mismatched_profile_build_keeps_facts_but_not_prewarm_tables() {
        assert!(shader_table_allowed("pso_manifest.tsv", true));
        assert!(shader_table_allowed("pso_pairs.tsv", true));
        assert!(shader_table_allowed("shader_pairs.tsv", true));
        assert!(shader_table_allowed("shader_facts.tsv", true));
        assert!(!shader_table_allowed("pso_manifest.tsv", false));
        assert!(!shader_table_allowed("pso_pairs.tsv", false));
        assert!(!shader_table_allowed("shader_pairs.tsv", false));
        assert!(shader_table_allowed("shader_facts.tsv", false));
        assert!(shader_table_allowed("shader_facts_manual.tsv", false));
    }

    #[test]
    fn profile_pairs_are_limited_to_the_actual_build_manifest() {
        let manifest = "# nxshaders 1\nkey\tstage\n1111111111111111\tvs\n2222222222222222\tps\n";
        let pairs = "# vertex / pixel\n1111111111111111\t2222222222222222\n1111111111111111\t3333333333333333\n";
        let (filtered, removed) = reconciled_pairs(manifest, pairs);
        assert_eq!(removed, 1);
        assert!(filtered.contains("1111111111111111\t2222222222222222"));
        assert!(!filtered.contains("3333333333333333"));
    }

    #[test]
    fn complete_live_variants_are_promoted_but_failed_attempts_are_not() {
        let root = std::env::temp_dir().join(format!("nxrs-live-promote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let good = root.join("live/0123456789abcdef");
        let bad = root.join("live/fedcba9876543210");
        std::fs::create_dir_all(good.join("spv")).unwrap();
        std::fs::create_dir_all(bad.join("spv")).unwrap();
        std::fs::write(root.join("manifest.tsv"), "# nxshaders 1\nkey\tstage\n").unwrap();
        std::fs::write(root.join("pairs.tsv"), "# nxshaders pairs 1\n").unwrap();
        std::fs::write(good.join("spv/0123456789abcdef.spv"), b"spv").unwrap();
        std::fs::write(good.join("spv/0123456789abcdef.json"), b"{}").unwrap();
        std::fs::write(good.join("manifest.tsv"), "0123456789abcdef\tps\n").unwrap();
        std::fs::write(
            good.join("pairs.tsv"),
            "0123456789abcdef\t1111111111111111\n",
        )
        .unwrap();
        std::fs::write(bad.join("spv/fedcba9876543210.json"), b"{}").unwrap();
        assert_eq!(super::promote_live_shaders(&root).unwrap(), 1);
        assert!(root.join("spv/0123456789abcdef.spv").is_file());
        assert!(!root.join("spv/fedcba9876543210.spv").is_file());
        assert!(std::fs::read_to_string(root.join("manifest.tsv"))
            .unwrap()
            .contains("0123456789abcdef\tps"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// nxnative DM_PROBE's syntax: <module>:<offset> pairs, hex; a probe line each for the translator.
    #[test]
    fn probe_points_from_the_environment() {
        assert_eq!(
            probe_lines("main:0x1234, sdk:abc0").unwrap(),
            vec!["probe main 0x1234", "probe sdk 0xabc0"]
        );
        assert!(probe_lines("").unwrap().is_empty());
        assert!(
            probe_lines("main:0x1235").is_err()
                && probe_lines("main").is_err()
                && probe_lines("main:zz").is_err()
        );
    }

    #[test]
    fn modules_c_exports_the_entry() {
        let s = modules_c(
            &[("main".into(), "0x7000000000".into(), "0x1000".into())],
            "nxrt_main",
            false,
        );
        assert!(s.contains(if cfg!(windows) {
            "__declspec(dllexport) int nxrs_game_main"
        } else {
            "__attribute__((visibility(\"default\"))) int nxrs_game_main"
        }));
        assert!(
            s.contains("{\"main\", 0x7000000000ull, 0x1000ull, main_fn_addrs, main_fn_ptrs, 0}")
        );
        assert!(s.contains("mods[0].count = main_fn_count;"));
        let w = modules_c(
            &[("main".into(), "0x7000000000".into(), "0x1000".into())],
            WEB_ENTRY,
            true,
        );
        assert!(w.contains("__attribute__((used, visibility(\"default\"))) int nxrs_game_main"));
        assert!(w.contains("return nxweb_main(argc, argv, mods, 1);"));
    }

    #[test]
    fn web_builds_go_beside_the_input() {
        let input = if cfg!(windows) {
            r"P:\Games\x\game.nsp"
        } else {
            "/games/x/game.nsp"
        };
        let want = if cfg!(windows) {
            r"P:\Games\x\nxrs-web\X"
        } else {
            "/games/x/nxrs-web/X"
        };
        assert_eq!(
            default_web_outdir(Path::new(input), "X"),
            PathBuf::from(want)
        );
    }
}

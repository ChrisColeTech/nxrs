//! `nxrs-build build`: package → data folder → C → shaders → code DLL + loader, in Turbo's output layout:
//!
//! ```text
//! <outdir>\<name>.exe            the loader (cc::STUB_C)
//! <outdir>\<name>_code.dll       the game's code, compiled, with the runtime linked in
//! <outdir>\<name>_data\          exefs\, the RomFS marker, control.nacp, the icon; DLC and saves later
//! <outdir>\<name>.ini            written once, when missing
//! <outdir>\<name>_build[_v<N>|_base]\gen   the generated C (kept: a rebuild regenerates only what changed)
//!                                     \obj   one object per chunk under its content key, link.rsp
//! ```
//!
//! `<outdir>` defaults to `<the input's folder>\SwitchRecomp Builds\<name>`, the folder the launcher builds into.

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
    /// `--chunk`: about this many ARM instructions per C file (Turbo's default 30000); None: the translator's sizing
    pub chunk: Option<u64>,
    /// `--native`: compile for this processor (-march=native) instead of x86-64-v3
    pub native: bool,
    pub no_shaders: bool,
    /// v1: the DLC hint names `--install-dlc` (what a v1 caller passes); v2: `nxrs-build dlc install`
    pub v1: bool,
    /// v1: the Turbo options that were passed and mean nothing here, said after the banner
    pub ignored: Option<String>,
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
    let s: String = title.nfkd().filter(|c| c.is_ascii() && (c.is_ascii_alphanumeric() || "-_ ".contains(*c))).collect();
    let s = s.trim();
    if s.is_empty() { "Game".into() } else { s.into() }
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
    let base = if input.is_file() { input.file_stem() } else { input.file_name() };
    base.map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Game".into())
}

pub fn default_outdir(input: &Path, name: &str) -> PathBuf {
    let abs = std::path::absolute(input).unwrap_or_else(|_| input.to_path_buf());
    abs.parent().unwrap_or(Path::new(".")).join("SwitchRecomp Builds").join(name)
}

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

fn module_lines(exefs: &BTreeMap<String, Vec<u8>>) -> Result<Vec<ModLine>> {
    let mut nsos = BTreeMap::new();
    for name in translator::loader::layout::order() {
        if let Some(raw) = exefs.get(&name) {
            nsos.insert(name.clone(), translator::loader::Nso::parse(raw).map_err(|e| anyhow!("{name}: {e}"))?);
        }
    }
    let lay = translator::loader::layout(&|n: &str| nsos.get(n).map(|x| x.size)).map_err(|e| anyhow!(e))?;
    Ok(lay
        .into_iter()
        .map(|m| {
            let n = &nsos[&m.name];
            ModLine { name: m.name.clone(), base: m.base, text: n.text.1, build_id: n.build_id.clone() }
        })
        .collect())
}

/// The modules layout.json lists, as (name, base, size) strings.
fn layout_modules(gen: &Path) -> Result<Vec<(String, String, String)>> {
    let text = std::fs::read_to_string(gen.join("layout.json")).with_context(|| format!("{}", gen.join("layout.json").display()))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let mut out = vec![];
    for m in v.as_array().ok_or_else(|| anyhow!("layout.json: not a list"))? {
        let f = |k: &str| m.get(k).and_then(|x| x.as_str()).map(String::from).ok_or_else(|| anyhow!("layout.json: no {k}"));
        out.push((f("name")?, f("base")?, f("size")?));
    }
    Ok(out)
}

/// modules.c: the module table for the engine and the DLL's entry point, which calls `entry` (`nxfront_main` or
/// `nxrt_main`, both `int (int argc, char **argv, const RtModule *mods, int count)`).
pub fn modules_c(mods: &[(String, String, String)], entry: &str) -> String {
    let mut s = String::from(
        "// Written by nxrs: the translated modules for nxrt_main, and the entry the loader calls.\n#include <stdlib.h>\n#include \"nxrt.h\"\n#include \"patches.h\"\n\n\
         volatile int g_fnprof __attribute__((visibility(\"hidden\")));\n\
         uint8_t rt_patch_on[256] __attribute__((visibility(\"hidden\")));\n\
         int nxfront_main(int argc, char **argv, const RtModule *mods, int count);\n\n\
         // A patch set is on by its default; its environment switch set to 0 turns it off, to anything else on.\n\
         static void nxrs_patch_sets(void) {\n\
         #define NXRS_SET(i, name, env, def) { const char *v = getenv(env); rt_patch_on[i] = (v && v[0]) ? (v[0] != '0') : (def); }\n\
         \x20   RT_PATCH_SETS(NXRS_SET)\n\
         #undef NXRS_SET\n\
         }\n\n",
    );
    for (n, _, _) in mods {
        s.push_str(&format!(
            "extern const uint64_t {n}_fn_addrs[];\nextern const GuestFn {n}_fn_ptrs[];\nextern const uint64_t {n}_fn_count;\n"
        ));
    }
    s.push_str("\n__declspec(dllexport) int nxrs_game_main(int argc, char **argv) {\n    nxrs_patch_sets();\n    RtModule mods[] = {\n");
    for (n, b, z) in mods {
        s.push_str(&format!("        {{\"{n}\", {b}ull, {z}ull, {n}_fn_addrs, {n}_fn_ptrs, 0}},\n"));
    }
    s.push_str("    };\n");
    for (i, (n, _, _)) in mods.iter().enumerate() {
        s.push_str(&format!("    mods[{i}].count = {n}_fn_count;\n"));
    }
    s.push_str(&format!("    return {entry}(argc, argv, mods, {});\n}}\n", mods.len()));
    s
}

/// The runtime's C interface (src/runtime/include/nxrt.h), carried by nxrs so a release needs only the libraries.
const NXRT_H: &str = include_str!("../../runtime/include/nxrt.h");

/// The compile units of a generated tree: every chunk and table of the modules in layout.json, and modules.c.
pub fn units(clang: &Clang, gen: &Path, obj: &Path, mods: &[(String, String, String)], engine: &cc::Engine) -> Result<Vec<Unit>> {
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
        .chain([format!("-I{}", include.display()), format!("-I{}", gen.display())])
        .collect();
    let mut v = vec![];
    for (m, _, _) in mods {
        let dir = gen.join(m);
        let mut srcs: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("{}", dir.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "c") && !p.file_name().unwrap().to_string_lossy().starts_with('.'))
            .collect();
        srcs.sort();
        for src in srcs {
            let stem = src.file_stem().unwrap().to_string_lossy().into_owned();
            v.push(Unit { obj: obj.join(format!("{stem}.o")), src, flags: gen_flags.clone(), headers: hk.clone() });
        }
    }
    crate::write_if_changed(&obj.join("nxrt.h"), NXRT_H.as_bytes())?;
    crate::write_if_changed(&obj.join("modules.c"), modules_c(mods, engine.entry()).as_bytes())?;
    let mut h2 = headers.clone();
    h2.push(obj.join("nxrt.h"));
    v.push(Unit {
        src: obj.join("modules.c"),
        obj: obj.join("modules.o"),
        flags: clang
            .gen_flags()
            .into_iter()
            .chain([format!("-I{}", obj.display()), format!("-I{}", include.display()), format!("-I{}", gen.display())])
            .collect(),
        headers: cc::headers_key(clang, &h2)?,
    });
    Ok(v)
}

/// The shader step: every shader the game ships, found and translated to SPIR-V into `<data>\shaders\`
/// (`shaders::prebuild_data`: the ExeFS, the RomFS the marker names and the add-on content installed there), and the
/// profile's shader tables (nxnative's prewarm seed, pairs and translation answers) into `<data>\shaders\profile\`.
/// A rebuild reuses every translated shader whose inputs and tools are unchanged.
fn shaders_step(out: &Out, data: &Path, profile: Option<&str>, a: &BuildArgs) -> Result<()> {
    if a.no_shaders {
        out.say("shaders: not translated (--no-shaders); a game built so draws only the shaders an earlier build left in its data folder");
        return Ok(());
    }
    out.progress("shaders", 0, 0);
    let prof_dir = data.join("shaders").join("profile");
    if let Some(id) = profile {
        let tables = profiles::shader_tables(id).map_err(|e| anyhow!(e))?;
        if !tables.is_empty() {
            std::fs::create_dir_all(&prof_dir)?;
            for (name, text) in &tables {
                crate::write_if_changed(&prof_dir.join(name), text.as_bytes())?;
            }
            let names: Vec<&str> = tables.iter().map(|(n, _)| n.as_str()).collect();
            out.say(&format!("shaders: the profile's tables ({}) in {}", names.join(", "), prof_dir.display()));
        }
    }
    let opts = shaders::Options { keys: a.keys.clone(), jobs: a.jobs.max(1), ..shaders::Options::default() };
    let say = |m: &str| out.say(&format!("  {m}"));
    out.say("Translating the shaders...");
    shaders::prebuild_data(data, &opts, &say).map_err(|e| anyhow!("shaders: {e} (--no-shaders builds without them)"))?;
    Ok(())
}

pub fn build(a: &BuildArgs, out: &Out) -> Result<Built> {
    let t_all = Instant::now();
    let mut timings = vec![];
    if !a.input.exists() {
        bail!("Cannot load the game: {} does not exist", a.input.display());
    }
    // the compiler and the runtime are checked first, not after minutes of translation
    let clang = if a.no_build { None } else { Some(cc::find_clang(a.cc.as_deref())?) };
    out.say("nxrs - use your own copy of the game. No games or keys included.");
    if let Some(i) = &a.ignored {
        out.say(i);
    }
    out.progress("open", 0, 0);
    let game = package::open_game(&a.input, &OpenOptions { keys: a.keys.clone(), update: a.update.clone() })
        .map_err(|e| anyhow!("Cannot load the game: {e}"))?;
    if game.exefs.files.is_empty() {
        bail!("Cannot load the game: {} has no ExeFS modules", a.input.display());
    }
    // a title on Nintendo's SDK runs on the whole engine; homebrew brings its own and gets the runtime alone
    let engine = match &clang {
        // a retail title (Nintendo's rtld and an NPDM beside its sdk); the runtime's own homebrew has neither
        Some(c) => Some(cc::find_engine(game.exefs.files.contains_key("sdk") && (game.exefs.files.contains_key("rtld") || game.exefs.files.contains_key("main.npdm")), c.gnu)?),
        None => None,
    };
    for m in module_lines(&game.exefs.files)? {
        out.say(&format!("  {:<8} {:010X}  code {:.1} MB  build id {}", m.name, m.base, m.text as f64 / 1048576.0, m.build_id));
    }
    let builds = crate::profile::module_builds(&game.exefs.files);
    let prof = crate::profile::find(game.exefs.title_id, &builds);
    out.say(&crate::profile::line(&prof, game.exefs.title_id, &builds));
    let patch_text = crate::profile::patch_table(&builds, out)?;
    let title = a
        .name
        .clone()
        .or_else(|| game.exefs.files.get("main.npdm").and_then(|n| npdm_name(n)))
        .unwrap_or_else(|| source_name(&a.input));
    let name = output_name(&title);
    let outdir = std::path::absolute(a.outdir.clone().unwrap_or_else(|| default_outdir(&a.input, &name)))?;
    std::fs::create_dir_all(&outdir).with_context(|| format!("creating {}", outdir.display()))?;
    let base = outdir.join(&name);
    let with = |suffix: &str| PathBuf::from(format!("{}{suffix}", base.display()));
    let (exe, dll, data) = (with(".exe"), with("_code.dll"), with("_data"));
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
    let mut opts = GenOptions { out: gen.clone(), jobs: a.jobs.max(1), emit: EmitOptions::default(), patches, ..GenOptions::default() };
    if let Some(n) = a.chunk.filter(|&n| n > 0) {
        // Turbo counts ARM instructions per C file; the translator sizes chunks by the C they make
        opts.chunk_bytes = n as i64 * translator::aot::discover::EST_INT;
        opts.chunk_fns = usize::MAX;
    }
    let key = gen_key(&game.exefs.files, patch_text.as_deref().unwrap_or(""), &format!("{:?}{:?}", (opts.chunk_fns, opts.chunk_bytes, opts.big_fn), (opts.emit.reg_locals, opts.emit.callee_saved)))?;
    let stamp = gen.join("nxrs.gen.key");
    let reuse = std::fs::read_to_string(&stamp).ok().as_deref() == Some(key.as_str()) && gen.join("layout.json").is_file();
    if reuse {
        let mods = layout_modules(&gen)?;
        let chunks: usize = mods
            .iter()
            .map(|(m, _, _)| std::fs::read_dir(gen.join(m)).map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".c")).count()).unwrap_or(0))
            .sum();
        out.say("Translating ARM64 to C...");
        out.say(&format!("  C generated: {chunks}/{chunks} chunks (unchanged since the last build)"));
    } else {
        let _ = std::fs::remove_file(&stamp);
        let started = std::sync::atomic::AtomicBool::new(false);
        let t_gen = Instant::now();
        let report = generate(&game.exefs.files, &opts, &|line: &str| {
            if let Some(rest) = line.strip_prefix("C generated: ") {
                if !started.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    out.say("Translating ARM64 to C...");
                }
                out.say(&format!("  C generated: {rest} chunks ({:.0}s)", t_gen.elapsed().as_secs_f64()));
                if let Some((d, n)) = rest.split_once('/') {
                    out.progress("translate", d.trim().parse().unwrap_or(0), n.trim().parse().unwrap_or(0));
                }
            } else {
                out.say(&format!("  {line}"));
            }
        })?;
        translator::aot::runtime_headers::write(&gen)?;
        let fns: usize = report.modules.iter().map(|m| m.functions).sum();
        let bytes: usize = report.modules.iter().map(|m| m.c_bytes).sum();
        let files: usize = report.modules.iter().map(|m| m.chunks + 1).sum();
        out.say(&format!("C code: {files} files, {:.1} MB, {fns} functions", bytes as f64 / 1e6));
        std::fs::write(&stamp, &key)?;
    }
    timings.push(("translate", t.elapsed().as_secs_f64()));

    // ---- data folder
    let t = Instant::now();
    let v1 = a.v1;
    game.write_data(&data, &|m: &str| {
        // the hint names the option the caller's contract has
        if v1 {
            out.say(&m.replace("install it with nxrs dlc install (the launcher does)", "install it with --install-dlc (the launcher does)"));
        } else {
            out.say(&m.replace("install it with nxrs dlc install", "install it with nxrs-build dlc install"));
        }
    })?;
    let ini = with(".ini");
    if !ini.exists() {
        std::fs::write(&ini, ini_text(&name))?;
    }
    timings.push(("data", t.elapsed().as_secs_f64()));
    if a.no_build {
        return Ok(Built { exe, data, timings });
    }
    let (clang, engine) = (clang.unwrap(), engine.unwrap());

    // ---- shaders
    let t = Instant::now();
    shaders_step(out, &data, prof.as_ref().map(|m| m.profile.id.as_str()), a)?;
    timings.push(("shaders", t.elapsed().as_secs_f64()));

    // ---- compile and link
    let t = Instant::now();
    let mods = layout_modules(&gen)?;
    let mut units = units(&clang, &gen, &obj, &mods, &engine)?;
    if a.native {
        for u in &mut units {
            for f in &mut u.flags {
                if f == "-march=x86-64-v3" {
                    *f = "-march=native".into();
                }
            }
        }
    }
    out.say(&format!(
        "Compiling with {} {} (-O1{}, {} at a time)...",
        clang.path.file_name().unwrap().to_string_lossy(),
        clang.version.trim_start_matches("clang version "),
        if a.native { ", -march=native" } else { "" },
        a.jobs.max(1)
    ));
    let total = units.len();
    let rep = cc::compile(&clang, &units, &build_dir, a.jobs.max(1), &|line: &str| {
        out.say(line);
        if let Some((d, n)) = line.trim().strip_prefix("compiled ").and_then(|r| r.split_once(' ')).and_then(|(dn, _)| dn.split_once('/')) {
            out.progress("compile", d.parse().unwrap_or(0), n.parse().unwrap_or(total));
        }
    })?;
    cc::remove_stale_objects(&obj, &units);
    out.say(&format!("  compile: {} of {} units compiled, the rest reused ({:.1}s)", rep.compiled, rep.units, rep.seconds));
    timings.push(("compile", t.elapsed().as_secs_f64()));
    let t = Instant::now();
    out.say(&format!("Linking {} ...", dll.file_name().unwrap().to_string_lossy()));
    out.progress("link", 0, 0);
    let objs: Vec<PathBuf> = units.iter().map(|u| u.obj.clone()).collect();
    out.say(&format!("  with {} ({})", engine.lib.file_name().unwrap().to_string_lossy(), engine.entry()));
    cc::link_dll(&clang, &objs, &engine, &obj, &dll)?;
    cc::link_exe(&clang, &obj, &exe)?;
    timings.push(("link", t.elapsed().as_secs_f64()));
    timings.push(("total", t_all.elapsed().as_secs_f64()));
    Ok(Built { exe, data, timings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names_are_ascii() {
        assert_eq!(output_name("Pokémon Legends: Arceus"), "Pokemon Legends Arceus");
        assert_eq!(output_name("  ★★  "), "Game");
        assert_eq!(output_name("Mario Kart 8 Deluxe 1G1U1D"), "Mario Kart 8 Deluxe 1G1U1D");
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
        let d = default_outdir(Path::new(r"P:\Dumps\X\x.nsp"), "X");
        assert_eq!(d, PathBuf::from(r"P:\Dumps\X\SwitchRecomp Builds\X"));
    }

    #[test]
    fn modules_c_exports_the_entry() {
        let s = modules_c(&[("main".into(), "0x7000000000".into(), "0x1000".into())], "nxrt_main");
        assert!(s.contains("__declspec(dllexport) int nxrs_game_main"));
        assert!(s.contains("{\"main\", 0x7000000000ull, 0x1000ull, main_fn_addrs, main_fn_ptrs, 0}"));
        assert!(s.contains("mods[0].count = main_fn_count;"));
    }
}

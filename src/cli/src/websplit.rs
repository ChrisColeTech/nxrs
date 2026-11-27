//! The web build's hot/cold split and optimization (docs/35 §4.6, WEB-W6), after the link:
//!
//! - **Hot set**: `<name>_data/hotset.txt`, the guest functions a run entered (`NXRS_FNPROF=<file>`, the runtime's
//!   recorder: one hex address a line, `# base ...` last). Recorded natively (`py scripts/play.py <scenario> --env
//!   NXRS_FNPROF=<file>`) and copied into the web build's data folder; the native modules sit at `FIRST_BASE`, the
//!   web build's at `WEB_FIRST_BASE` with the same offsets, so an address maps by that difference to the web
//!   module's `f_<hex>`. No hot set: no split (one module, as before W6).
//! - **Optimization**: `wasm-opt` from emsdk's binaryen on the linked module (`NXRS_WEB_OPT`: `0` none, `1` (default)
//!   or `2` for `-O1` / `-O2`).
//! - **Split**: emcc links with `-sSPLIT_MODULE` (its loader: a cold function's table slot holds a placeholder that
//!   loads the cold module synchronously on the calling thread, then calls it: a cold call waits, never traps) and
//!   function names; `wasm-split` moves every guest function outside the hot set into `game.cold.wasm`. Only guest
//!   functions (`f_<hex>`) move: they run only on the game's threads (pthreads, where a synchronous fetch and compile
//!   are allowed), never on the page's thread. Names are stripped from both modules; `game.symbols` keeps them.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The cold module beside `game.wasm` (Emscripten's split loader names it from the placeholder namespace).
pub const COLD: &str = "game.cold.wasm";
/// Both modules' function names (wasm-split's symbol maps), for reading a stack.
pub const SYMBOLS: &str = "game.symbols";
/// The hot set in the data folder.
pub const HOTSET: &str = "hotset.txt";
/// The placeholder import namespace: Emscripten's loader maps `placeholder.<id>` to `game.<id>.wasm`.
const PLACEHOLDER_NS: &str = "placeholder.cold";

/// The web names (`f_<hex>`) of a recorded hot set: native addresses moved from `native_base` to `web_base`.
pub fn hot_names(text: &str, native_base: u64, web_base: u64) -> HashSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| u64::from_str_radix(l, 16).ok())
        .filter(|&a| a >= native_base)
        .map(|a| format!("f_{:x}", a - native_base + web_base))
        .collect()
}

/// The guest functions of an Emscripten symbol map (`index:name` lines) outside `hot`, in map order.
pub fn cold_names(symbols: &str, hot: &HashSet<String>) -> Vec<String> {
    symbols
        .lines()
        .filter_map(|l| l.split_once(':').map(|(_, n)| n.trim()))
        .filter(|n| is_guest(n) && !hot.contains(*n))
        .map(str::to_string)
        .collect()
}

/// `f_<hex>`: a translated guest function.
fn is_guest(n: &str) -> bool {
    n.strip_prefix("f_")
        .is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The optimization level (`NXRS_WEB_OPT`).
pub fn opt_level() -> u32 {
    std::env::var("NXRS_WEB_OPT").ok().and_then(|v| v.parse().ok()).map_or(1, |v: u32| v.min(2))
}

/// The hot set of a web build's data folder, when it was recorded.
pub fn hot_set(data: &Path) -> Option<String> {
    std::fs::read_to_string(data.join(HOTSET)).ok()
}

fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let o = cmd.output().with_context(|| format!("running {what}"))?;
    if !o.status.success() {
        bail!("{what} failed: {}", String::from_utf8_lossy(&o.stderr).chars().rev().take(3000).collect::<String>().chars().rev().collect::<String>());
    }
    Ok(())
}

/// After a split link (`game.wasm.orig`, the instrumented `game.wasm` emcc also writes, and `game.js.symbols`):
/// optimizes, splits into `game.wasm` + `game.cold.wasm` with stripped names and writes `game.symbols`. Returns
/// lines for the build log.
pub fn split(outdir: &Path, emsdk: &Path, hot: &str, native_base: u64, web_base: u64) -> Result<Vec<String>> {
    let bin = emsdk.join("upstream").join("bin");
    let exe = |n: &str| bin.join(if cfg!(windows) { format!("{n}.exe") } else { n.to_string() });
    let wasm = outdir.join("game.wasm");
    let orig = outdir.join("game.wasm.orig");
    let map = outdir.join("game.js.symbols");
    let mut said = vec![];
    let hot = hot_names(hot, native_base, web_base);
    let symbols = std::fs::read_to_string(&map).context("reading game.js.symbols")?;
    let cold = cold_names(&symbols, &hot);
    let guests = symbols.lines().filter(|l| l.split_once(':').is_some_and(|(_, n)| is_guest(n))).count();
    let cold_list = outdir.join("game.cold.txt");
    std::fs::write(&cold_list, cold.join("\n"))?;
    // the optimized, still named module the split reads
    let lvl = opt_level();
    let input: PathBuf = if lvl > 0 {
        let t = std::time::Instant::now();
        let opt = outdir.join("game.opt.wasm");
        run(
            Command::new(exe("wasm-opt"))
                .arg(&orig)
                .arg(format!("-O{lvl}"))
                .arg("--debuginfo")
                .arg("-o")
                .arg(&opt),
            "wasm-opt",
        )?;
        said.push(format!(
            "  wasm-opt -O{lvl}: {:.1} -> {:.1} MiB in {:.1}s",
            mb(&orig),
            mb(&opt),
            t.elapsed().as_secs_f64()
        ));
        opt
    } else {
        orig.clone()
    };
    let t = std::time::Instant::now();
    let cold_path = outdir.join(COLD);
    run(
        Command::new(exe("wasm-split"))
            .arg("--split")
            .arg(&input)
            .arg(format!("--split-funcs=@{}", cold_list.display()))
            .arg("-o1")
            .arg(&wasm)
            .arg("-o2")
            .arg(&cold_path)
            .arg(format!("--placeholder-namespace={PLACEHOLDER_NS}"))
            .arg("--import-namespace=primary")
            .arg("--symbolmap")
            .arg("--strip-debug"),
        "wasm-split",
    )?;
    said.push(format!(
        "  split: {} of {guests} guest functions hot ({} in the recording); game.wasm {:.1} MiB, {COLD} {:.1} MiB in {:.1}s",
        guests - cold.len(),
        hot.len(),
        mb(&wasm),
        mb(&cold_path),
        t.elapsed().as_secs_f64()
    ));
    // wasm-split's symbol maps (`<output>.symbols`), joined
    let mut joined = String::new();
    for (name, p) in [("game.wasm", &wasm), (COLD, &cold_path)] {
        let s = sym_path(p);
        if let Ok(t) = std::fs::read_to_string(&s) {
            joined.push_str(&format!("# {name}\n{t}"));
            let _ = std::fs::remove_file(&s);
        }
    }
    std::fs::write(outdir.join(SYMBOLS), joined)?;
    for p in [orig, map, cold_list, outdir.join("game.opt.wasm")] {
        let _ = std::fs::remove_file(p);
    }
    Ok(said)
}

fn sym_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".symbols");
    PathBuf::from(s)
}

fn mb(p: &Path) -> f64 {
    std::fs::metadata(p).map_or(0.0, |m| m.len() as f64 / 1048576.0)
}

/// Removes what an earlier split build left that this one (unsplit) does not make.
pub fn clean_unsplit(outdir: &Path) {
    for n in [COLD, SYMBOLS, "game.cold.wasm.gz", "game.wasm.orig"] {
        let _ = std::fs::remove_file(outdir.join(n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_set_maps_native_addresses_to_web_names() {
        let rec = "7000000030\n7005c00010\n\n6fffffff00\n# base 0x7000000000 outside 0\n";
        let h = hot_names(rec, 0x70_0000_0000, 0x2_0000_0000);
        let mut v: Vec<_> = h.into_iter().collect();
        v.sort();
        assert_eq!(v, ["f_200000030", "f_205c00010"]);
    }

    #[test]
    fn only_guest_functions_outside_the_hot_set_go_cold() {
        let map = "0:opus_decoder_create\n1:f_200000000\n2:f_200000030\n3:f_\n4:f_main_thing\n5:nxweb_main\n6:f_205c00010\n";
        let hot: HashSet<String> = ["f_200000030".to_string()].into();
        assert_eq!(cold_names(map, &hot), ["f_200000000", "f_205c00010"]);
    }
}

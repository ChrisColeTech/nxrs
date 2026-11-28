//! `nxrs-build`: the engine's command line (the launcher runs it).
//!
//! Contract v1 (the Qt launcher's legacy command line, docs/LAUNCHER_CONTRACT.md):
//!
//!   nxrs-build <input> --outdir <dir> -o <name> [--jobs N] [--update <pkg> | --no-update] [ignored: --target --backend
//!        --cc -O --runtime --native --chunk]
//!   nxrs-build <game> --update-info <package>...
//!   nxrs-build --install-dlc <pkg>... | --sync-dlc [<pkg>...] | --remove-dlc <id or pkg>... | --list-dlc  --datadir <dir>
//!
//! Contract v2 (subcommands; `--machine` adds `@progress` / `@result` lines):
//!
//!   nxrs-build build <input> [-o <name>] [--outdir <dir>] [--jobs N (the CPU count)] [--update <pkg> | --no-update] [--keys <prod.keys>]
//!        <input> is a Switch package or folder (.nsp, .nup, .xci, .nro, exefs/) or a decrypted 3DS
//!        dump (.3ds .cia .cxi .app; translate-only in the first waves: `--no-build` stages the C
//!        and the data folder, a full build says what is not built yet)
//!   nxrs-build update-info [--game <game>] <package>...
//!   nxrs-build dlc install|sync|remove|list --datadir <dir> [package or title id...]
//!   nxrs-build profile <title id> [<main build id>]
//!   nxrs-build doctor | contract
//!   nxrs-build shader-xlate <shaders folder>     (a web build's server: one run-time shader from stdin, translated
//!                                                 into <shaders folder>\live\<key>\; the key on stdout)
//!   nxrs-build translate <input> --out <dir>      (development: the C and the data folder only, as nxrt-dev uses it)
//!
//! Exit codes: 0 done, 1 failed (the reason on stderr, or the UPDATE / DLC error lines), 2 bad arguments.

mod cc;
mod cheats;
mod fpsfind;
mod mods;
mod out;
mod pipeline;
mod priority;
mod profile;
mod shaderimport;
mod shaderxlate;
mod stubgen;
mod webhost;
mod webpack;
mod webshaders;
mod websplit;
mod webzip;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use out::Out;
use std::path::{Path, PathBuf};
use translator::package::{self, dlc, UpdateChoice};

/// Writes `data` unless the file holds it already (an unchanged file keeps its time).
pub fn write_if_changed(p: &Path, data: &[u8]) -> std::io::Result<bool> {
    translator::aot::gen::write_if_changed(p, data)
}

pub const CONTRACT: &str = "nxrs-contract 2";

// ------------------------------------------------------------------------------------------------ contract v1

/// The legacy launcher's build options, as it passes them.
#[derive(Parser, Debug)]
#[command(
    name = "nxrs-build",
    about = "Builds a Switch game into a native x86-64 program for this system, Windows or Linux (contract v1; `nxrs-build help` for the subcommands)."
)]
struct V1 {
    /// game folder (exefs/ + romfs/), .nsp or .nup package, or a decrypted 3DS dump (.3ds .cia .cxi)
    input: Option<PathBuf>,
    /// output name (no extension)
    #[arg(short = 'o', long = "out")]
    out: Option<String>,
    /// output folder (default: <the input's folder>\nxrs\<name>)
    #[arg(long)]
    outdir: Option<PathBuf>,
    /// C compiler: used when it is a clang, ignored otherwise (nxrs compiles with clang)
    #[arg(long)]
    cc: Option<String>,
    /// ignored: the game code is compiled at one measured flag set
    #[arg(short = 'O')]
    opt: Option<String>,
    #[arg(long, default_value_t = pipeline::default_jobs())]
    jobs: usize,
    /// ARM instructions per C file (default: the translator sizes its chunks itself)
    #[arg(long)]
    chunk: Option<u64>,
    /// windows | linux: nxrs-build builds for the system it runs on (a Linux build runs nxrs-build on Linux); the
    /// other one is refused
    #[arg(long)]
    target: Option<String>,
    /// ignored
    #[arg(long)]
    backend: Option<String>,
    /// optimize for THIS processor (-march=native; the default is x86-64-v3: AVX2, FMA, BMI2)
    #[arg(long)]
    native: bool,
    /// only generate the C code and stage the game data
    #[arg(long)]
    no_build: bool,
    /// do not translate the game's shaders during the build
    #[arg(long)]
    no_shaders: bool,
    #[arg(long, hide = true)]
    keep_c: bool,
    /// (diagnostics) translate no function: the runtime interprets the whole game
    #[arg(long, hide = true)]
    interp_only: bool,
    /// ignored: the runtime is linked into the game's code DLL
    #[arg(long)]
    runtime: Option<String>,
    /// check the prebuilt runtime library and the compiler, and exit
    #[arg(long)]
    build_runtime: bool,
    #[arg(long, num_args = 1.., value_name = "PACKAGE")]
    install_dlc: Option<Vec<PathBuf>>,
    #[arg(long, num_args = 0.., value_name = "PACKAGE")]
    sync_dlc: Option<Vec<PathBuf>>,
    #[arg(long, num_args = 1.., value_name = "TITLE_OR_PACKAGE")]
    remove_dlc: Option<Vec<String>>,
    #[arg(long)]
    list_dlc: bool,
    /// the built game's data folder (<outdir>\<name>_data), for the DLC options
    #[arg(long)]
    datadir: Option<PathBuf>,
    #[arg(long, hide = true)]
    dlc_stub: bool,
    /// apply a game update from a package of its own (.nsp .xci, or .nca with its .tik beside it)
    #[arg(long, value_name = "PACKAGE", conflicts_with = "no_update")]
    update: Option<PathBuf>,
    /// build a combined package (game + update) without the update it holds
    #[arg(long)]
    no_update: bool,
    /// say what each update package is (title, version) and exit; with the game as the input, also check it belongs
    #[arg(long, num_args = 1.., value_name = "PACKAGE")]
    update_info: Option<Vec<PathBuf>>,
    #[arg(short, long)]
    quiet: bool,
    /// prod.keys (default: NXRS_PROD_KEYS, else the launcher's keys folder)
    #[arg(long)]
    keys: Option<PathBuf>,
    /// print the contract version and exit
    #[arg(long)]
    contract: bool,
}

fn update_choice(update: &Option<PathBuf>, no_update: bool) -> UpdateChoice {
    match (update, no_update) {
        (Some(p), _) => UpdateChoice::Package(p.clone()),
        (None, true) => UpdateChoice::None,
        _ => UpdateChoice::Embedded,
    }
}

fn usage_error(msg: &str) -> i32 {
    eprintln!("nxrs-build: error: {msg}");
    2
}

fn run_v1(args: Vec<String>) -> i32 {
    let a = match V1::try_parse_from(&args) {
        Ok(a) => a,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    let o = Out::new(a.quiet, false);
    if a.contract {
        o.print(CONTRACT);
        return 0;
    }
    if a.build_runtime {
        let gnu = cc::use_gnu();
        return match (cc::find_runtime_lib(gnu), cc::find_clang(a.cc.as_deref())) {
            (Ok(lib), Ok(c)) => {
                o.say(&format!(
                    "The runtime ships prebuilt and is linked into each game: {} ({})",
                    lib.display(),
                    c.version
                ));
                0
            }
            (Err(e), _) | (_, Err(e)) => fail(&e),
        };
    }
    if a.install_dlc.is_some() || a.sync_dlc.is_some() || a.remove_dlc.is_some() || a.list_dlc {
        let datadir = a.datadir.clone().or_else(|| match (&a.outdir, &a.out) {
            (Some(d), Some(n)) => Some(
                std::path::absolute(d)
                    .unwrap_or(d.clone())
                    .join(format!("{n}_data")),
            ),
            _ => None,
        });
        let Some(datadir) = datadir else {
            return usage_error("the DLC options need --datadir (or --outdir with -o)");
        };
        let say = |m: &str| o.say(m);
        let r = (|| -> Result<Vec<String>, package::PackageError> {
            let mut errors = vec![];
            for w in a.remove_dlc.iter().flatten() {
                if dlc::remove(&datadir, w, &say)? == 0 {
                    errors.push(format!("{w} is not installed"));
                    say(&format!("DLC error: {w} is not installed"));
                }
            }
            if let Some(p) = &a.sync_dlc {
                errors.extend(dlc::sync(p, &datadir, &say, a.keys.as_deref())?.1);
            }
            if let Some(p) = &a.install_dlc {
                errors.extend(dlc::install(p, &datadir, &say, a.keys.as_deref(), a.dlc_stub)?.1);
            }
            if a.list_dlc {
                dlc::describe(&datadir, &|m: &str| o.print(m));
            }
            Ok(errors)
        })();
        return match r {
            Ok(e) if e.is_empty() => 0,
            Ok(_) => 1,
            Err(e) => {
                eprintln!("DLC error: {e}");
                1
            }
        };
    }
    if let Some(pkgs) = &a.update_info {
        return update_info(&o, pkgs, a.input.as_deref(), a.keys.as_deref());
    }
    let Some(input) = a.input.clone() else {
        return usage_error("the game folder, .nup or .nsp package is required");
    };
    let host = if cfg!(target_os = "linux") {
        "linux"
    } else {
        "windows"
    };
    if a.target
        .as_deref()
        .is_some_and(|t| (t == "linux" || t == "windows") && t != host)
    {
        return fail(&anyhow!(
            "--target {}: nxrs-build builds {host} x86-64 programs here; build on {} for that",
            a.target.as_deref().unwrap_or(""),
            a.target.as_deref().unwrap_or("")
        ));
    }
    let ignored: Vec<String> = [
        a.target
            .as_ref()
            .filter(|v| v.as_str() != host)
            .map(|v| format!("--target {v}")),
        a.backend.as_ref().map(|v| format!("--backend {v}")),
        a.cc.as_ref()
            .filter(|c| !c.to_lowercase().contains("clang"))
            .map(|v| format!("--cc {v}")),
        a.opt.as_ref().map(|v| format!("-O {v}")),
        a.runtime.as_ref().map(|v| format!("--runtime {v}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let b = pipeline::BuildArgs {
        input,
        name: a.out.clone(),
        outdir: a.outdir.clone(),
        jobs: a.jobs,
        update: update_choice(&a.update, a.no_update),
        keys: a.keys.clone(),
        cc: a.cc.clone(),
        no_build: a.no_build,
        no_shaders: a.no_shaders,
        chunk: a.chunk,
        native: a.native,
        v1: true,
        ignored: (!ignored.is_empty()).then(|| {
            format!(
                "nxrs-build: ignored (options with no meaning here): {}",
                ignored.join(" ")
            )
        }),
        interp_only: a.interp_only,
        web: false,
        shaders_from: None,
    };
    build(&o, &b)
}

fn fail(e: &anyhow::Error) -> i32 {
    eprintln!("{e:#}");
    1
}

/// The other programs of a multi-program application, built beside program 0 so oe::ExecuteProgram finds them:
/// (index, `<name>_p<index>`) for every index but 0. Nothing when a program was named (`--program`).
pub fn other_programs(indices: &[u64], name: &str, chosen: bool) -> Vec<(u64, String)> {
    if chosen {
        return vec![];
    }
    indices
        .iter()
        .filter(|&&i| i != 0)
        .map(|&i| (i, format!("{name}_p{i}")))
        .collect()
}

fn build(o: &Out, b: &pipeline::BuildArgs) -> i32 {
    match pipeline::build(b, o) {
        Ok(built) => {
            let t: Vec<String> = built
                .timings
                .iter()
                .map(|(k, v)| format!("{k} {v:.1}s"))
                .collect();
            o.say(&format!("  time: {}", t.join(", ")));
            if b.no_build {
                o.say(&format!("Done (C and data only): {}", built.data.display()));
            } else {
                o.say(&format!("Done: {}", built.exe.display()));
            }
            // a multi-program application (the profile's `programs`, the package's Program NCAs): every program,
            // each `<name>_p<N>` in the same folder (nxnative builds them all into one app)
            let chosen = std::env::var("NXRS_PROGRAM_INDEX").is_ok_and(|v| !v.trim().is_empty());
            let indices =
                package::program_indices(&b.input, b.keys.as_deref()).unwrap_or_else(|_| vec![0]);
            let name = built
                .exe
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            for (i, pname) in other_programs(&indices, &name, chosen) {
                o.say(&format!(
                    "Program {i} of {} (started by the game with oe::ExecuteProgram): {pname}",
                    indices.len()
                ));
                std::env::set_var("NXRS_PROGRAM_INDEX", i.to_string());
                let pb = pipeline::BuildArgs {
                    name: Some(pname),
                    outdir: built.exe.parent().map(Path::to_path_buf),
                    ..b.clone()
                };
                let r = pipeline::build(&pb, o);
                std::env::remove_var("NXRS_PROGRAM_INDEX");
                if let Err(e) = r {
                    o.result(
                        false,
                        &format!("program {i}: {e:#}").replace(['\r', '\n'], " "),
                    );
                    return fail(&e);
                }
            }
            o.result(true, &built.exe.display().to_string());
            0
        }
        Err(e) => {
            o.result(false, &format!("{e:#}").replace(['\r', '\n'], " "));
            fail(&e)
        }
    }
}

fn update_info(o: &Out, packages: &[PathBuf], game: Option<&Path>, keys: Option<&Path>) -> i32 {
    let mut failed = false;
    for p in packages {
        match package::update_info(p, keys, game) {
            Ok(info) => o.print(&info.line()),
            Err(e) => {
                failed = true;
                o.print(&format!("UPDATE error: {e}"));
            }
        }
    }
    i32::from(failed)
}

// ------------------------------------------------------------------------------------------------ contract v2

#[derive(Parser)]
#[command(
    name = "nxrs-build",
    about = "nxrs engine: builds Switch games into native programs, and translates decrypted 3DS dumps (.3ds .cia .cxi) the same way"
)]
struct V2 {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone, Default)]
struct PkgArgs {
    /// prod.keys (default: NXRS_PROD_KEYS, else %LOCALAPPDATA%\nxrs\keys\prod.keys, then %LOCALAPPDATA%\RecompLauncher\RecompLauncher\keys\prod.keys)
    #[arg(long)]
    keys: Option<PathBuf>,
    /// apply the game update in this package of its own (.nsp, .xci, or .nca with its .tik beside it)
    #[arg(long, conflicts_with = "no_update")]
    update: Option<PathBuf>,
    /// leave out the update a combined package holds
    #[arg(long)]
    no_update: bool,
    /// build program N of a multi-program application (title id + N; the game starts it with oe::ExecuteProgram);
    /// name it `<name>_p<N>` beside program 0's `<name>` so the engine finds it
    #[arg(long)]
    program: Option<u32>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build a game: <outdir>\<name>.exe, <name>_code.dll and <name>_data\. Switch packages and
    /// folders build and play; a decrypted 3DS dump (.3ds .cia .cxi .app) translates and stages
    /// its data folder (--no-build) until the 3DS engine runtime is built.
    Build {
        input: PathBuf,
        #[arg(short = 'o', long = "out")]
        out: Option<String>,
        #[arg(long)]
        outdir: Option<PathBuf>,
        #[arg(long, default_value_t = pipeline::default_jobs())]
        jobs: usize,
        #[command(flatten)]
        pkg: PkgArgs,
        /// clang to compile with (default: NXRS_CLANG, the bundled one, LLVM's folder, PATH)
        #[arg(long)]
        cc: Option<String>,
        #[arg(long)]
        no_build: bool,
        #[arg(long)]
        no_shaders: bool,
        /// ARM instructions per C file (default: the translator sizes its chunks)
        #[arg(long)]
        chunk: Option<u64>,
        /// compile for this processor (-march=native) instead of x86-64-v3
        #[arg(long)]
        native: bool,
        #[arg(short, long)]
        quiet: bool,
        /// also print @progress and @result lines
        #[arg(long)]
        machine: bool,
        /// (diagnostics) translate no function: the runtime interprets the whole game
        #[arg(long, hide = true)]
        interp_only: bool,
        /// `web`: a browser build (wasm + JS, docs/35) into <input folder>\nxrs-web\<name> instead of a Windows build
        #[arg(long, value_parser = ["web"])]
        target: Option<String>,
        /// another build's data folder: its translated shaders (those its runs translated live) are imported, for a
        /// title that makes its shaders at run time; a web build cannot translate them in the browser
        #[arg(long)]
        shaders_from: Option<PathBuf>,
    },
    /// Say what each update package is; with --game, check it belongs to that game (exit 1 on any error).
    UpdateInfo {
        #[arg(required = true)]
        packages: Vec<PathBuf>,
        #[arg(long)]
        game: Option<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    /// Add-on content of a built game.
    Dlc {
        #[command(subcommand)]
        cmd: DlcCmd,
    },
    /// The game profile of a title (and main build id), or of a package's program (--package).
    Profile {
        title_id: Option<String>,
        main_build_id: Option<String>,
        #[arg(long)]
        package: Option<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    /// Check this machine and the toolchain.
    Doctor,
    /// Print the contract version.
    Contract,
    /// Translate one shader a web build's page asked for (stdin: u32 LE ctl length, ctl, code) into
    /// <shaders>\live\<key>\ as a native run's live translator does; prints the key.
    ShaderXlate { shaders: PathBuf },
    /// Drive the pads' rumble and lights for a web page (stdin: `pad <active> <low> <high> x4` lines from the
    /// launcher's server) with the engine's native input layer; quiet and exit at the end of stdin.
    PadBridge,
    /// (development) Translate to C and stage the data folder: what nxrt-dev's end-to-end tests run.
    Translate {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        data: Option<PathBuf>,
        #[command(flatten)]
        pkg: PkgArgs,
        #[arg(long, default_value_t = pipeline::default_jobs())]
        jobs: usize,
    },
}

#[derive(Subcommand)]
enum DlcCmd {
    Install {
        #[arg(long)]
        datadir: PathBuf,
        #[arg(required = true)]
        packages: Vec<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    Sync {
        #[arg(long)]
        datadir: PathBuf,
        packages: Vec<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    Remove {
        #[arg(long)]
        datadir: PathBuf,
        #[arg(required = true)]
        what: Vec<String>,
    },
    List {
        #[arg(long)]
        datadir: PathBuf,
    },
}

const SUBCOMMANDS: &[&str] = &[
    "build",
    "update-info",
    "dlc",
    "profile",
    "doctor",
    "contract",
    "shader-xlate",
    "pad-bridge",
    "translate",
    "help",
];

fn run_v2(args: Vec<String>) -> i32 {
    let a = match V2::try_parse_from(&args) {
        Ok(a) => a,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    match a.cmd {
        Cmd::Contract => {
            out::line(CONTRACT);
            0
        }
        Cmd::Build {
            input,
            out,
            outdir,
            jobs,
            pkg,
            cc,
            no_build,
            no_shaders,
            chunk,
            native,
            quiet,
            machine,
            interp_only,
            target,
            shaders_from,
        } => {
            if let Some(n) = pkg.program {
                std::env::set_var("NXRS_PROGRAM_INDEX", n.to_string());
            }
            let o = Out::new(quiet, machine);
            let b = pipeline::BuildArgs {
                input,
                name: out,
                outdir,
                jobs,
                update: update_choice(&pkg.update, pkg.no_update),
                keys: pkg.keys,
                cc,
                no_build,
                no_shaders,
                chunk,
                native,
                v1: false,
                ignored: None,
                interp_only,
                web: target.as_deref() == Some("web"),
                shaders_from,
            };
            build(&o, &b)
        }
        Cmd::UpdateInfo {
            packages,
            game,
            keys,
        } => update_info(&Out::default(), &packages, game.as_deref(), keys.as_deref()),
        Cmd::Dlc { cmd } => {
            let o = Out::default();
            let say = |m: &str| o.say(m);
            let r = (|| -> Result<Vec<String>, package::PackageError> {
                Ok(match cmd {
                    DlcCmd::Install {
                        datadir,
                        packages,
                        keys,
                    } => dlc::install(&packages, &datadir, &say, keys.as_deref(), false)?.1,
                    DlcCmd::Sync {
                        datadir,
                        packages,
                        keys,
                    } => dlc::sync(&packages, &datadir, &say, keys.as_deref())?.1,
                    DlcCmd::Remove { datadir, what } => {
                        let mut errors = vec![];
                        for w in &what {
                            if dlc::remove(&datadir, w, &say)? == 0 {
                                errors.push(format!("{w} is not installed"));
                                say(&format!("DLC error: {w} is not installed"));
                            }
                        }
                        errors
                    }
                    DlcCmd::List { datadir } => {
                        dlc::describe(&datadir, &say);
                        vec![]
                    }
                })
            })();
            match r {
                Ok(e) if e.is_empty() => 0,
                Ok(_) => 1,
                Err(e) => {
                    eprintln!("DLC error: {e}");
                    1
                }
            }
        }
        Cmd::Profile {
            title_id,
            main_build_id,
            package,
            keys,
        } => {
            match profile::command(
                title_id.as_deref(),
                main_build_id.as_deref(),
                package.as_deref(),
                keys.as_deref(),
            ) {
                Ok(()) => 0,
                Err(e) => fail(&e),
            }
        }
        Cmd::Doctor => doctor(),
        Cmd::ShaderXlate { shaders } => match shaderxlate::command(&shaders) {
            Ok(key) => {
                out::line(&key);
                0
            }
            Err(e) => fail(&e),
        },
        Cmd::PadBridge => pad_bridge(),
        Cmd::Translate {
            input,
            out,
            data,
            pkg,
            jobs,
        } => match translate(&input, &out, data, &pkg, jobs) {
            Ok(()) => 0,
            Err(e) => fail(&e),
        },
    }
}

fn translate(
    input: &Path,
    out: &Path,
    data: Option<PathBuf>,
    pkg: &PkgArgs,
    jobs: usize,
) -> Result<()> {
    let game = package::open_game(
        input,
        &package::OpenOptions {
            keys: pkg.keys.clone(),
            update: update_choice(&pkg.update, pkg.no_update),
        },
    )?;
    let opts = translator::aot::GenOptions {
        out: out.to_path_buf(),
        jobs,
        ..Default::default()
    };
    let report = translator::aot::generate(&game.exefs.files, &opts, &|l| eprintln!("{l}"))?;
    translator::aot::runtime_headers::write(out)?;
    let datadir = data.unwrap_or_else(|| out.join("data"));
    game.write_data(&datadir, &|m: &str| out::line(m))?;
    out::line(&format!(
        "translated {} functions in {} modules to {} ({:.1}s); data in {}",
        report.modules.iter().map(|m| m.functions).sum::<usize>(),
        report.modules.len(),
        out.display(),
        report.seconds,
        datadir.display()
    ));
    Ok(())
}

/// `pad-bridge` (WEB-PADBRIDGE): the engine's input layer drives the pads for a web page (os `input::bridge`).
#[cfg(windows)]
fn pad_bridge() -> i32 {
    nxos::input::bridge::run()
}

#[cfg(not(windows))]
fn pad_bridge() -> i32 {
    eprintln!("pad-bridge: not built on this platform (Windows only); the page keeps its own rumble");
    1
}

fn doctor() -> i32 {
    let mut bad = 0;
    let mut check = |ok: bool, what: String| {
        out::line(&format!("{} {what}", if ok { "ok  " } else { "FAIL" }));
        bad += i32::from(!ok);
    };
    #[cfg(target_arch = "x86_64")]
    check(
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("bmi2"),
        "CPU: AVX2, FMA, BMI2 (the game code is built for x86-64-v3)".into(),
    );
    #[cfg(target_arch = "aarch64")]
    check(
        std::arch::is_aarch64_feature_detected!("fp16")
            && std::arch::is_aarch64_feature_detected!("rcpc")
            && std::arch::is_aarch64_feature_detected!("dotprod")
            && std::arch::is_aarch64_feature_detected!("crc"),
        "CPU: FP16, RCpc, dot product, CRC32 (the game code is built for ARMv8.2-A)".into(),
    );
    let gnu = cc::use_gnu();
    out::line(&format!(
        "     toolchain: {} (NXRS_TOOLCHAIN=msvc|gnu decides)",
        if cfg!(target_os = "linux") {
            if cfg!(target_arch = "aarch64") {
                "clang + ld.lld for aarch64-unknown-linux-gnu, the engine's libnxfront.a / libnxrt.a"
            } else {
                "clang + ld.lld for x86_64-unknown-linux-gnu, the engine's libnxfront.a / libnxrt.a"
            }
        } else if gnu {
            "llvm-mingw + the gnullvm engine (no Visual Studio)"
        } else {
            "clang + Visual Studio's libraries (MSVC ABI)"
        }
    ));
    match cc::find_clang(None) {
        Ok(c) => check(
            true,
            format!(
                "compiler: {} ({}, {})",
                c.path.display(),
                c.version,
                c.target()
            ),
        ),
        Err(e) => check(false, format!("compiler: {e}")),
    }
    if cfg!(target_os = "linux") {
        match cc::find_clang(None) {
            Ok(c) => match cc::find_lld(&c) {
                Some(l) => check(true, format!("linker: {}", l.display())),
                None => check(false, "linker: ld.lld not found beside clang".into()),
            },
            Err(_) => check(false, "linker: no clang to drive ld.lld".into()),
        }
    } else if !gnu {
        check(
            cc::have_visual_studio(),
            "Visual Studio C++ build tools (the MSVC libraries)".into(),
        );
    }
    match cc::find_runtime_lib(gnu) {
        Ok(p) => check(true, format!("runtime library: {}", p.display())),
        Err(e) => check(false, format!("runtime library: {e}")),
    }
    match cc::find_engine(true, gnu) {
        Ok(e) => check(true, format!("engine library: {}", e.lib.display())),
        Err(e) => check(false, format!("engine library: {e}")),
    }
    if cfg!(target_os = "linux") {
        linux_checks(&mut check);
    } else {
        let sys = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| r"C:\Windows".into());
        check(
            sys.join(r"System32\vulkan-1.dll").is_file(),
            "Vulkan loader (vulkan-1.dll)".into(),
        );
    }
    match vulkan_devices() {
        Ok(devs) if devs.is_empty() => check(false, "Vulkan: no device".into()),
        Ok(devs) => {
            for (name, api, driver, discrete) in &devs {
                let ok = *api >= ash::vk::API_VERSION_1_3;
                check(
                    ok,
                    format!(
                        "Vulkan device: {name}{} - Vulkan {}.{}.{}{}, driver {driver}",
                        if *discrete { " (discrete)" } else { "" },
                        ash::vk::api_version_major(*api),
                        ash::vk::api_version_minor(*api),
                        ash::vk::api_version_patch(*api),
                        if ok {
                            ""
                        } else {
                            " (the engine needs Vulkan 1.3: update the graphics driver)"
                        }
                    ),
                );
            }
        }
        Err(e) => check(false, format!("Vulkan: {e}")),
    }
    for (what, dir, need) in disk_checks() {
        match free_bytes(&dir) {
            Some(free) => check(
                free >= need,
                format!(
                    "free disk for {what} ({}): {:.1} GB{}",
                    dir.display(),
                    free as f64 / 1e9,
                    if free >= need {
                        String::new()
                    } else {
                        format!(" (a build needs about {:.0} GB)", need as f64 / 1e9)
                    }
                ),
            ),
            None => check(
                false,
                format!("free disk for {what}: {} cannot be read", dir.display()),
            ),
        }
    }
    match translator::package::default_keys_path() {
        Some(p) => check(p.is_file(), format!("keys: {} (present; never read here)", p.display())),
        None => check(false, "keys: none installed (NXRS_PROD_KEYS or the launcher's keys folder); NSP games need them".into()),
    }
    out::line(
        "info  3DS: decrypted dumps (.3ds .cia .cxi) need no keys; encrypted ones are refused until a decryption lane exists",
    );
    i32::from(bad > 0)
}

/// What a game on Linux loads at run time: the Vulkan loader, the shared libraries the engine links (sound, codecs),
/// a display, and the optional pieces (SDL2 for controllers, zenity / kdialog for dialogs, ffmpeg for F7).
fn linux_checks(check: &mut dyn FnMut(bool, String)) {
    let multiarch = if cfg!(target_arch = "aarch64") {
        "aarch64-linux-gnu"
    } else {
        "x86_64-linux-gnu"
    };
    let dirs = [
        format!("/usr/lib/{multiarch}"),
        "/usr/lib64".into(),
        "/usr/lib".into(),
        format!("/lib/{multiarch}"),
        "/lib64".into(),
    ];
    let has_lib = |prefix: &str| {
        dirs.iter().any(|d| {
            std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(prefix))
        })
    };
    let on_path = |n: &str| {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(n).is_file()))
    };
    check(
        has_lib("libvulkan.so.1"),
        "Vulkan loader (libvulkan.so.1)".into(),
    );
    check(
        has_lib("libasound.so.2"),
        "ALSA (sound; PulseAudio / PipeWire through its default device): libasound.so.2".into(),
    );
    if cc::STATIC_CODECS {
        out::line("     codecs: opus, libvpx, dav1d, FFmpeg (H.264, AAC) built into the engine");
    } else {
        for (lib, what) in [
            ("libopus.so.0", "libopus (Opus sound)"),
            ("libvpx.so", "libvpx (VP8 / VP9 movies)"),
            ("libdav1d.so", "dav1d (AV1 movies)"),
            ("libavcodec.so", "FFmpeg libavcodec (H.264 movies, AAC)"),
            ("libavutil.so", "FFmpeg libavutil"),
        ] {
            check(has_lib(lib), format!("{what}: {lib}"));
        }
    }
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .map(|v| format!("Wayland {}", v.to_string_lossy()))
        .or_else(|| std::env::var_os("DISPLAY").map(|v| format!("X11 {}", v.to_string_lossy())));
    check(
        display.is_some(),
        format!(
            "display: {}",
            display.unwrap_or_else(|| {
                "none (DISPLAY / WAYLAND_DISPLAY unset): a game cannot open its window".into()
            })
        ),
    );
    out::line(&format!(
        "     controllers: SDL2 {}",
        if has_lib("libSDL2-2.0.so.0") {
            "found (Xbox and other pads)"
        } else {
            "not found (install libsdl2: only native HID pads and the keyboard)"
        }
    ));
    out::line(&format!(
        "     dialogs: {}",
        if on_path("zenity") {
            "zenity"
        } else if on_path("kdialog") {
            "kdialog"
        } else {
            "neither zenity nor kdialog (messages and the keyboard go to the log)"
        }
    ));
    out::line(&format!(
        "     recording (F7): {}",
        if on_path("ffmpeg") {
            "ffmpeg found"
        } else {
            "ffmpeg not found (F7 needs it)"
        }
    ));
}

/// Every Vulkan device: (name, API version, driver version as its vendor writes it, discrete).
fn vulkan_devices() -> Result<Vec<(String, u32, String, bool)>> {
    use ash::vk;
    // SAFETY: the loader is loaded and released here; every handle made is destroyed before return
    unsafe {
        let entry =
            ash::Entry::load().map_err(|e| anyhow!("the Vulkan loader does not load: {e}"))?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
        let inst = entry
            .create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .map_err(|e| anyhow!("no Vulkan instance: {e}"))?;
        let mut out = vec![];
        for pd in inst.enumerate_physical_devices().unwrap_or_default() {
            let p = inst.get_physical_device_properties(pd);
            let name = p
                .device_name_as_c_str()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let d = p.driver_version;
            let driver = if p.vendor_id == 0x10DE {
                // NVIDIA: 10 bits major, 8 minor, 8 sub-minor, 6 patch
                format!("{}.{}", d >> 22, (d >> 14) & 0xFF)
            } else if p.vendor_id == 0x8086 && cfg!(windows) {
                format!("{}.{}", d >> 14, d & 0x3FFF)
            } else {
                format!(
                    "{}.{}.{}",
                    vk::api_version_major(d),
                    vk::api_version_minor(d),
                    vk::api_version_patch(d)
                )
            };
            out.push((
                name,
                p.api_version,
                driver,
                p.device_type == vk::PhysicalDeviceType::DISCRETE_GPU,
            ));
        }
        inst.destroy_instance(None);
        Ok(out)
    }
}

/// (what, folder, bytes a build needs): the folder nxrs builds into by default (beside nxrs: the launcher's engine
/// folder) and the temporary folder. A game's generated C, objects and code DLL take up to ~3 GB (Pokemon Legends:
/// Z-A), its translated shaders a few hundred MB.
fn disk_checks() -> Vec<(&'static str, PathBuf, u64)> {
    let mut v = vec![];
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        v.push(("builds", d, 4_000_000_000));
    }
    v.push(("temporary files", std::env::temp_dir(), 1_000_000_000));
    v
}

/// Free bytes for this user on the volume of `dir` (statvfs: the blocks available to an unprivileged user).
#[cfg(unix)]
fn free_bytes(dir: &Path) -> Option<u64> {
    let p = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { core::mem::zeroed() };
    // SAFETY: a NUL-terminated path and our output variable
    (unsafe { libc::statvfs(p.as_ptr(), &mut st) } == 0)
        .then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

/// Free bytes for this user on the volume of `dir`.
#[cfg(windows)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated path and our output variable
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            w.as_ptr(),
            &mut free,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

fn main() {
    // below normal before anything else, so every tool the build starts inherits it (WEB-WFREEZE)
    priority::lower();
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some(s) if SUBCOMMANDS.contains(&s) => run_v2(args),
        _ => run_v1(args),
    };
    std::process::exit(code);
}

#[cfg(test)]
mod program_tests {
    /// Program 0's build makes the others beside it as `<name>_p<N>` (the names oe::ExecuteProgram starts); a build of
    /// one named program (`--program`) makes only that one.
    #[test]
    fn other_programs_are_built_beside_program_0() {
        assert_eq!(
            super::other_programs(&[0, 1], "Sonic", false),
            vec![(1, "Sonic_p1".to_string())]
        );
        assert_eq!(super::other_programs(&[0, 1, 2], "3DAS", false).len(), 2);
        assert!(super::other_programs(&[0], "MK8D", false).is_empty());
        assert!(super::other_programs(&[0, 1], "Sonic", true).is_empty());
    }
}

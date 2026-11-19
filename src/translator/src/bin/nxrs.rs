//! `nxrs`: the engine's command line: package input, updates and DLC in place, and the ARM64 -> C translator.
//!
//!   nxrs translate <input> --out <dir> [--data <dir>] [--update <pkg> | --no-update] [--keys prod.keys] [--jobs 4]
//!   nxrs extract   <input> --out <dir> [--update <pkg> | --no-update]   write the ExeFS files to <dir>
//!   nxrs info      <input>                                               package listing, ExeFS, build ids
//!   nxrs update-info [--game <input>] <package>...                       one UPDATE line a package (Turbo's)
//!   nxrs romfs ls|cat|sha256 <input> [path...] [--update <pkg> | --no-update] [--dlc <title id>]
//!   nxrs dlc install|sync|remove|list --datadir <dir> [package or title id...]
//!
//! <input> is a dumped folder (exefs/ + romfs/), an .nsp (plain, encrypted, combined 1G+1U(+1D)), a .nup, or for
//! `romfs`, a built game's data folder (its romfs.nsp / romfs.nup / romfs.dir marker). Packages are only read.

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use translator::aot::emit::EmitOptions;
use translator::aot::{generate, GenOptions};
use translator::package::{self, dlc, OpenOptions, RomFs, UpdateChoice};

#[derive(Parser)]
#[command(name = "nxrs", version, about = "RecompLauncher engine: package input and ARM64 -> C translation")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone, Default)]
struct Pkg {
    /// prod.keys (default: SWR_PROD_KEYS, else %LOCALAPPDATA%\RecompLauncher\RecompLauncher\keys\prod.keys)
    #[arg(long, global = true)]
    keys: Option<PathBuf>,
    /// Apply the game update in this package of its own (.nsp, .xci, or .nca with its .tik beside it)
    #[arg(long, conflicts_with = "no_update")]
    update: Option<PathBuf>,
    /// Leave out the update a combined package holds (the game as released)
    #[arg(long)]
    no_update: bool,
}

impl Pkg {
    fn options(&self) -> OpenOptions {
        let update = match (&self.update, self.no_update) {
            (Some(p), _) => UpdateChoice::Package(p.clone()),
            (None, true) => UpdateChoice::None,
            _ => UpdateChoice::Embedded,
        };
        OpenOptions { keys: self.keys.clone(), update }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Translate every function of the input's ExeFS modules to C, and stage the game's data folder.
    Translate {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// The data folder to stage (exefs/, the RomFS marker, control.nacp); default <out>/data
        #[arg(long)]
        data: Option<PathBuf>,
        #[command(flatten)]
        pkg: Pkg,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Comma-separated module names (default: all)
        #[arg(long, default_value = "")]
        modules: String,
        /// Emit only the first N functions of each module
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long, default_value_t = 3000)]
        chunk_fns: usize,
        #[arg(long, default_value_t = 3_000_000)]
        chunk_bytes: i64,
        #[arg(long, default_value_t = 300_000)]
        big_fn: i64,
        /// Code patch table (mkw-recomp patches.py format)
        #[arg(long)]
        patches: Option<PathBuf>,
        /// Guest registers kept in C locals per function (0: none)
        #[arg(long, default_value_t = 6)]
        reg_locals: usize,
        /// Reload X19-X29 and SP after every call too
        #[arg(long)]
        no_callee_saved_trust: bool,
    },
    /// Write the ExeFS files to a folder.
    Extract {
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        pkg: Pkg,
    },
    /// Show the package and its ExeFS.
    Info {
        input: PathBuf,
        #[command(flatten)]
        pkg: Pkg,
    },
    /// Say what each update package is; with --game, also check that it belongs to that game (exit 1 on any error).
    UpdateInfo {
        packages: Vec<PathBuf>,
        /// The game's own package (NSP)
        #[arg(long)]
        game: Option<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    /// Read a RomFS in place: list a directory, print a file, or hash files.
    Romfs {
        #[command(subcommand)]
        cmd: RomfsCmd,
    },
    /// Add-on content of a built game.
    Dlc {
        #[command(subcommand)]
        cmd: DlcCmd,
    },
}

#[derive(Args)]
struct RomfsSource {
    /// A folder, .nsp, .nup, or a built game's data folder
    input: PathBuf,
    #[command(flatten)]
    pkg: Pkg,
    /// With a data folder: the RomFS of this installed add-on title instead of the game's
    #[arg(long)]
    dlc: Option<String>,
}

#[derive(Subcommand)]
enum RomfsCmd {
    /// List a directory (default the root); --recursive lists every file with its size
    Ls {
        #[command(flatten)]
        src: RomfsSource,
        #[arg(default_value = "")]
        path: String,
        #[arg(long, short)]
        recursive: bool,
    },
    /// Write a file (or a range of it) to stdout or --out
    Cat {
        #[command(flatten)]
        src: RomfsSource,
        path: String,
        #[arg(long, default_value_t = 0)]
        offset: u64,
        #[arg(long)]
        len: Option<u64>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// SHA-256 of files (all of them with --all)
    Sha256 {
        #[command(flatten)]
        src: RomfsSource,
        paths: Vec<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum DlcCmd {
    /// Add the add-on content of these packages (.nsp .nsz .xci .xcz .nca)
    Install {
        #[arg(long)]
        datadir: PathBuf,
        #[arg(required = true)]
        packages: Vec<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
        /// (diagnostics) install a title without its data when the data cannot be read
        #[arg(long, hide = true)]
        dlc_stub: bool,
    },
    /// Make the installed add-on content exactly that of these packages (none: remove everything)
    Sync {
        #[arg(long)]
        datadir: PathBuf,
        packages: Vec<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    /// Remove titles by id or by the package they came from
    Remove {
        #[arg(long)]
        datadir: PathBuf,
        #[arg(required = true)]
        what: Vec<String>,
    },
    /// List the installed add-on content
    List {
        #[arg(long)]
        datadir: PathBuf,
    },
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn open(input: &Path, pkg: &Pkg) -> Result<package::Game> {
    let game = package::open_game(input, &pkg.options()).with_context(|| format!("opening {}", input.display()))?;
    let fs = &game.exefs;
    let tid = fs.title_id.map_or("unknown".into(), |t| format!("{t:016X}"));
    let ver = fs.version.map_or("unknown".into(), |v| v.to_string());
    println!("Analyzing {} ({:?}, title {tid}, version {ver}, update {:?})", input.display(), fs.kind, game.applied);
    for (name, data) in &fs.files {
        let bid = package::nso_build_id(data).map(|b| format!("  build id {}", hex(&b))).unwrap_or_default();
        println!("  exefs/{name:<10} {:>10} bytes{bid}", data.len());
    }
    Ok(game)
}

fn romfs_of(src: &RomfsSource) -> Result<RomFs> {
    let is_data = ["romfs.nsp", "romfs.nup", "romfs.dir"].iter().any(|m| src.input.join(m).is_file()) || src.dlc.is_some();
    if is_data {
        if let Some(id) = &src.dlc {
            let id = u64::from_str_radix(id, 16).context("--dlc takes a 16-digit title id")?;
            return Ok(dlc::title_romfs(&src.input, id, src.pkg.keys.as_deref())?);
        }
        return Ok(package::open_data_dir(&src.input, src.pkg.keys.as_deref())?);
    }
    let game = package::open_game(&src.input, &src.pkg.options())?;
    Ok(std::sync::Arc::try_unwrap(game.romfs).map_err(|_| anyhow::anyhow!("shared RomFS"))?)
}

/// One line as Turbo's Python prints it: text mode, so CRLF on Windows (the launcher compares the bytes).
fn say(m: &str) {
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "{m}{nl}");
    let _ = out.flush();
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Info { input, pkg } => {
            for line in package::list(&input, pkg.keys.as_deref())? {
                println!("{line}");
            }
            let g = open(&input, &pkg)?;
            println!("  romfs: {} files ({})", g.romfs.walk().len(), g.romfs.source);
        }
        Cmd::Extract { input, out, pkg } => {
            let g = open(&input, &pkg)?;
            std::fs::create_dir_all(&out)?;
            for (name, data) in &g.exefs.files {
                std::fs::write(out.join(name), data)?;
            }
            println!("wrote {} files to {}", g.exefs.files.len(), out.display());
        }
        Cmd::UpdateInfo { packages, game, keys } => {
            let mut failed = false;
            for p in &packages {
                match package::update_info(p, keys.as_deref(), game.as_deref()) {
                    Ok(info) => say(&info.line()),
                    Err(e) => {
                        failed = true;
                        say(&format!("UPDATE error: {e}"));
                    }
                }
            }
            if failed {
                std::process::exit(1);
            }
        }
        Cmd::Romfs { cmd } => match cmd {
            RomfsCmd::Ls { src, path, recursive } => {
                let r = romfs_of(&src)?;
                if recursive {
                    for (p, size) in r.walk() {
                        println!("{size:>12}  {p}");
                    }
                } else {
                    for e in r.list(&path)? {
                        println!("{:>12}  {}{}", if e.is_dir { "<dir>".into() } else { e.size.to_string() }, e.name, if e.is_dir { "/" } else { "" });
                    }
                }
            }
            RomfsCmd::Cat { src, path, offset, len, out } => {
                let r = romfs_of(&src)?;
                let data = r.read(&path, offset, len.unwrap_or(u64::MAX))?;
                match out {
                    Some(o) => std::fs::write(o, data)?,
                    None => std::io::stdout().write_all(&data)?,
                }
            }
            RomfsCmd::Sha256 { src, paths, all } => {
                let r = romfs_of(&src)?;
                let paths: Vec<String> = if all { r.walk().into_iter().map(|x| x.0).collect() } else { paths };
                for p in paths {
                    let size = r.find(&p).map(|n| n.size()).ok_or_else(|| anyhow::anyhow!("{p}: no such file"))?;
                    let mut h = Sha256::new();
                    let mut at = 0;
                    while at < size {
                        let part = r.read(&p, at, 16 << 20)?;
                        at += part.len() as u64;
                        h.update(&part);
                    }
                    println!("{}  {size}  {p}", hex(&h.finalize()));
                }
            }
        },
        Cmd::Dlc { cmd } => {
            let result = (|| -> std::result::Result<Vec<String>, package::PackageError> {
                Ok(match cmd {
                    DlcCmd::Install { datadir, packages, keys, dlc_stub } => {
                        dlc::install(&packages, &datadir, &say, keys.as_deref(), dlc_stub)?.1
                    }
                    DlcCmd::Sync { datadir, packages, keys } => dlc::sync(&packages, &datadir, &say, keys.as_deref())?.1,
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
            match result {
                Ok(errors) if errors.is_empty() => {}
                Ok(_) => std::process::exit(1),
                Err(e) => {
                    eprintln!("DLC error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Translate {
            input,
            out,
            data,
            pkg,
            jobs,
            modules,
            limit,
            chunk_fns,
            chunk_bytes,
            big_fn,
            patches,
            reg_locals,
            no_callee_saved_trust,
        } => {
            let game = open(&input, &pkg)?;
            let opts = GenOptions {
                out: out.clone(),
                jobs,
                modules: modules.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                limit,
                chunk_fns,
                chunk_bytes,
                big_fn,
                patches,
                emit: EmitOptions { reg_locals, callee_saved: !no_callee_saved_trust },
            };
            let report = generate(&game.exefs.files, &opts, &|line| eprintln!("{line}"))?;
            let headers = translator::aot::runtime_headers::write(&out)?;
            let datadir = data.unwrap_or_else(|| out.join("data"));
            game.write_data(&datadir, &say)?;
            println!(
                "translated {} functions in {} modules to {} ({:.1}s); {headers} runtime headers; data in {}",
                report.modules.iter().map(|m| m.functions).sum::<usize>(),
                report.modules.len(),
                out.display(),
                report.seconds,
                datadir.display()
            );
            if game.exefs.files.is_empty() {
                bail!("no ExeFS");
            }
        }
    }
    Ok(())
}

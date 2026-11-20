//! nxshaders: the shaders build step on its own, for testing.
//!
//!     nxshaders prebuild <game> --out <dir> [--jobs 4] [--scan-jobs 8] [--keys prod.keys] [--update <pkg> | --no-update]
//!                        [--limit N] [--scan-only] [--optimize]
//!     nxshaders stage <game> --data <dir> [--dlc <pkg>...]   a data folder as the build stages it (exefs, RomFS marker, DLC)
//!     nxshaders prebuild-data <data> [--jobs 4]              the build's call: <data>/shaders from <data>
//!     nxshaders tex-census <export dir> [--keys N]           texture instructions whose LOD clamp / residency the
//!                                                 translator drops (nxnative tex_census.py)
//!     nxshaders coverage <out dir> <used>...     how many of the keys a game used (Turbo's <game>_nvn folders:
//!                                                 vk/pipelines.tsv and cache/dumps) the build found and translated

use clap::{Parser, Subcommand};
use shaders::prebuild::{prebuild, Options};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use translator::package::UpdateChoice;

#[derive(Parser)]
#[command(name = "nxshaders", about = "Find every shader of a Switch game and translate it to SPIR-V")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Find, translate, validate and write a game's shaders.
    Prebuild {
        game: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        #[arg(long, default_value_t = 8)]
        scan_jobs: usize,
        #[arg(long)]
        keys: Option<PathBuf>,
        #[arg(long)]
        update: Option<PathBuf>,
        #[arg(long)]
        no_update: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        scan_only: bool,
        /// Let the translator run its optimizer (DebugMode off).
        #[arg(long)]
        optimize: bool,
        #[arg(long, default_value_t = 64)]
        batch: usize,
    },
    /// Keys a game used at run time (Turbo's `<game>_nvn` folders) against a build's manifest.
    Coverage { out: PathBuf, used: Vec<PathBuf> },
    /// The texture instructions whose LOD clamp or residency predicate the translator drops, over an `export` folder.
    TexCensus {
        dir: PathBuf,
        /// shader keys listed per finding
        #[arg(long, default_value_t = 10)]
        keys: usize,
    },
    /// Stage a data folder as the build does (exefs/, the RomFS marker), with add-on content installed from packages.
    Stage {
        game: PathBuf,
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        dlc: Vec<PathBuf>,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    /// The build's call: <data>/shaders from a data folder.
    PrebuildData {
        data: PathBuf,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
    /// Write found shaders as `<key>.ctl` / `<key>.code` + index.tsv (a sample spread over the keys), and with
    /// --exefs the raw ExeFS files: inputs for the cross-check and for debugging.
    Export {
        game: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        sample: Option<usize>,
        /// Only shaders of this architecture (sm86 or maxwell).
        #[arg(long)]
        arch: Option<String>,
        #[arg(long)]
        exefs: bool,
        #[arg(long)]
        keys: Option<PathBuf>,
    },
}

fn used_keys(dir: &Path) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut pipes = BTreeSet::new();
    let mut dumps = BTreeSet::new();
    let is_key = |s: &str| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if let Ok(t) = std::fs::read_to_string(dir.join("vk").join("pipelines.tsv")) {
        for l in t.lines() {
            for c in l.split('\t').skip(1).take(5) {
                if is_key(c) {
                    pipes.insert(c.to_ascii_lowercase());
                }
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir.join("cache").join("dumps")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(k) = n.strip_suffix(".ctl") {
                if is_key(k) {
                    dumps.insert(k.to_ascii_lowercase());
                }
            }
        }
    }
    (pipes, dumps)
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Prebuild { game, out, jobs, scan_jobs, keys, update, no_update, limit, scan_only, optimize, batch } => {
            let opts = Options {
                keys,
                update: if no_update {
                    UpdateChoice::None
                } else if let Some(u) = update {
                    UpdateChoice::Package(u)
                } else {
                    UpdateChoice::Embedded
                },
                jobs,
                scan_jobs,
                scan_only,
                limit,
                job: shaders::job::JobOptions { debug: !optimize, reduced: true },
                batch,
                facts_dir: None,
            };
            let say = |s: &str| println!("{s}");
            match prebuild(&game, &out, &opts, &say) {
                Ok(r) => {
                    println!(
                        "RESULT found={} translated={} reused={} failed={} skipped={} rt_in_library={} valid={} invalid={} scan_problems={} conflicts={} pairs={} files={} read_mb={:.0} scan_s={:.1} translate_s={:.1} validate_s={:.1} total_s={:.1}",
                        r.found, r.translated, r.reused, r.failed, r.skipped, r.in_library, r.valid, r.invalid, r.scan_problems, r.conflicts, r.pairs, r.files, r.bytes_read as f64 / 1e6, r.scan_seconds, r.translate_seconds, r.validate_seconds, r.total_seconds
                    );
                    println!("RESULT stages {:?}", r.by_stage);
                }
                Err(e) => {
                    eprintln!("nxshaders: {e}");
                    std::process::exit(1);
                }
            }
        }
        Cmd::Export { game, out, sample, arch, exefs, keys } => {
            let g = translator::package::open_game(&game, &translator::package::OpenOptions { keys, update: UpdateChoice::Embedded }).unwrap_or_else(|e| {
                eprintln!("{}: {e}", game.display());
                std::process::exit(1)
            });
            std::fs::create_dir_all(&out).unwrap();
            if exefs {
                for (n, d) in &g.exefs.files {
                    std::fs::write(out.join(format!("exefs_{n}")), d).unwrap();
                }
            }
            let say = |s: &str| eprintln!("{s}");
            let rep = shaders::scan::scan(&g.exefs.files, &g.romfs, 8, &say);
            let mut all: Vec<&shaders::scan::Shader> = rep.shaders.values().filter(|s| match arch.as_deref() {
                Some("sm86") => shaders::nvn::nvn2_ctl(&s.ctl),
                Some("maxwell") => !shaders::nvn::nvn2_ctl(&s.ctl),
                _ => true,
            }).collect();
            if let Some(n) = sample {
                let step = (all.len() / n.max(1)).max(1);
                all = all.into_iter().step_by(step).take(n).collect();
            }
            let mut idx = String::from("key	stage	source
");
            for s in &all {
                std::fs::write(out.join(format!("{}.ctl", s.key)), &s.ctl).unwrap();
                std::fs::write(out.join(format!("{}.code", s.key)), &s.code).unwrap();
                idx += &format!("{}	{}	{}
", s.key, s.stage, s.source);
            }
            std::fs::write(out.join("index.tsv"), idx).unwrap();
            println!("exported {} of {} shaders", all.len(), rep.shaders.len());
        }
        Cmd::Stage { game, data, dlc, keys } => {
            let say = |s: &str| println!("{s}");
            let g = translator::package::open_game(&game, &translator::package::OpenOptions { keys: keys.clone(), update: UpdateChoice::Embedded }).unwrap_or_else(|e| {
                eprintln!("{}: {e}", game.display());
                std::process::exit(1)
            });
            std::fs::create_dir_all(&data).unwrap();
            g.write_data(&data, &say).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1)
            });
            if !dlc.is_empty() {
                match translator::package::dlc::install(&dlc, &data, &say, keys.as_deref(), false) {
                    Ok((ids, _)) => println!("installed {} add-on title(s)", ids.len()),
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1)
                    }
                }
            }
        }
        Cmd::PrebuildData { data, jobs, keys } => {
            let say = |s: &str| println!("{s}");
            let opts = Options { keys, jobs, ..Options::default() };
            match shaders::prebuild_data(&data, &opts, &say) {
                Ok(r) => println!("RESULT found={} translated={} reused={} failed={} valid={} invalid={} total_s={:.1}", r.found, r.translated, r.reused, r.failed, r.valid, r.invalid, r.total_seconds),
                Err(e) => {
                    eprintln!("nxshaders: {e}");
                    std::process::exit(1)
                }
            }
        }
        Cmd::TexCensus { dir, keys } => match shaders::census::census_dir(&dir, keys) {
            Ok(r) => print!("{r}"),
            Err(e) => {
                eprintln!("nxshaders: {e}");
                std::process::exit(1)
            }
        },
        Cmd::Coverage { out, used } => {
            let manifest = std::fs::read_to_string(out.join("manifest.tsv")).unwrap_or_else(|e| {
                eprintln!("{}: {e}", out.display());
                std::process::exit(1)
            });
            let mut status: BTreeMap<String, String> = BTreeMap::new();
            for l in manifest.lines().filter(|l| !l.starts_with('#') && !l.starts_with("key\t")) {
                let c: Vec<&str> = l.split('\t').collect();
                if c.len() > 4 {
                    status.insert(c[0].to_string(), c[4].to_string());
                }
            }
            let mut all_p = BTreeSet::new();
            let mut all_d = BTreeSet::new();
            for u in &used {
                let (p, d) = used_keys(u);
                all_p.extend(p);
                all_d.extend(d);
            }
            let all: BTreeSet<String> = all_p.union(&all_d).cloned().collect();
            for (name, set) in [("pipelines", &all_p), ("dumps", &all_d), ("all", &all)] {
                let found = set.iter().filter(|k| status.contains_key(*k)).count();
                let ok = set.iter().filter(|k| status.get(*k).map(String::as_str) == Some("ok")).count();
                println!("COVERAGE {name}: used {} found {found} ok {ok} missing {}", set.len(), set.len() - found);
            }
            let missing: Vec<&String> = all.iter().filter(|k| !status.contains_key(*k)).collect();
            for k in missing.iter().take(40) {
                println!("MISSING {k}");
            }
        }
    }
}

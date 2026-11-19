//! The generator driver: layout, patch table, discovery per module, then parallel chunk emission, function
//! tables and manifests. Chunks are emitted by worker threads.

use crate::aot::discover::{discover, estimate_sizes, partition, DiscoverStats};
use crate::aot::emit::{table_text, EmitOptions, Emitter, GENERATED_MARKER};
use crate::aot::patches::Table;
use anyhow::{anyhow, Context, Result};
use crate::loader::layout::{layout, layout_h, layout_json};
use crate::loader::{Module, Nso};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct GenOptions {
    pub out: PathBuf,
    pub jobs: usize,
    /// Modules to emit; empty = every module of the ExeFS.
    pub modules: Vec<String>,
    /// Emit only the first N functions of each module (0 = all).
    pub limit: usize,
    pub chunk_fns: usize,
    pub chunk_bytes: i64,
    pub big_fn: i64,
    pub patches: Option<PathBuf>,
    pub emit: EmitOptions,
}

impl Default for GenOptions {
    fn default() -> Self {
        GenOptions {
            out: PathBuf::from("generated"),
            jobs: 4,
            modules: vec![],
            limit: 0,
            chunk_fns: 3000,
            chunk_bytes: 3_000_000,
            big_fn: 300_000,
            patches: None,
            emit: EmitOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModuleReport {
    pub name: String,
    pub build_id: String,
    pub functions: usize,
    pub chunks: usize,
    pub c_bytes: usize,
    pub discover: DiscoverStats,
    pub stats: BTreeMap<&'static str, u64>,
    pub unsupported: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default)]
pub struct GenReport {
    pub modules: Vec<ModuleReport>,
    pub seconds: f64,
}

/// Writes the file unless it already holds exactly `data` (an unchanged chunk keeps its mtime, so a build skips
/// it); a changed one goes to a temporary name first and is renamed into place.
pub fn write_if_changed(path: &Path, data: &[u8]) -> std::io::Result<bool> {
    if let Ok(old) = std::fs::read(path) {
        if old == data {
            return Ok(false);
        }
    }
    let tmp = path.with_file_name(format!(".{}.{}.tmp", path.file_name().unwrap().to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(true)
}

/// Python text-mode newline translation (CRLF on Windows).
fn text_mode(s: &str) -> String {
    if cfg!(windows) { s.replace('\n', "\r\n") } else { s.to_string() }
}

struct Task {
    module: usize,
    number: usize,
    path: PathBuf,
    fns: Vec<(u64, u64)>,
}

struct ChunkResult {
    module: usize,
    number: usize,
    file: String,
    functions: usize,
    bytes: usize,
    seconds: f64,
    stats: BTreeMap<&'static str, u64>,
    unsupported: BTreeMap<String, u64>,
}

/// Generates C for the ExeFS modules given as name -> NSO bytes.
pub fn generate(exefs: &BTreeMap<String, Vec<u8>>, opts: &GenOptions, progress: &(dyn Fn(&str) + Sync)) -> Result<GenReport> {
    let t0 = Instant::now();
    let mut nsos: HashMap<String, Nso> = HashMap::new();
    for name in crate::loader::layout::order() {
        if let Some(raw) = exefs.get(&name) {
            nsos.insert(name.clone(), Nso::parse(raw).with_context(|| format!("loading {name}"))?);
        }
    }
    let mods = layout(&|n: &str| nsos.get(n).map(|x| x.size)).map_err(|e| anyhow!(e))?;
    std::fs::create_dir_all(&opts.out)?;
    // gen.py writes these two with Path.write_text, so they carry the platform's line ends.
    write_if_changed(&opts.out.join("layout.json"), text_mode(&layout_json(&mods)).as_bytes())?;
    write_if_changed(&opts.out.join("layout.h"), text_mode(&layout_h(&mods)).as_bytes())?;
    let table = match &opts.patches {
        Some(p) => Table::load(p, &mods)?,
        None => Table::empty(&mods)?,
    };
    write_if_changed(&opts.out.join("patches.h"), table.header_text(GENERATED_MARKER).as_bytes())?;
    let want: Vec<String> =
        if opts.modules.is_empty() { mods.iter().map(|m| m.name.clone()).collect() } else { opts.modules.clone() };
    for w in &want {
        if !mods.iter().any(|m| &m.name == w) {
            return Err(anyhow!("not in the ExeFS: {w}"));
        }
    }

    let mut modules: Vec<Module> = vec![];
    let mut starts_abs: Vec<Vec<u64>> = vec![];
    let mut emitted: Vec<Vec<u64>> = vec![];
    let mut dstats: Vec<DiscoverStats> = vec![];
    let mut nchunks: Vec<usize> = vec![];
    let mut tasks: Vec<(i64, Task)> = vec![];
    for ml in &mods {
        if !want.contains(&ml.name) {
            continue;
        }
        let t = Instant::now();
        let nso = nsos.remove(&ml.name).unwrap();
        let m = Module::new(&ml.name, nso, ml.base);
        table.check(&m, &ml.name)?;
        let (starts, ds) = discover(&m);
        let est = estimate_sizes(&m, &starts);
        let abs: Vec<u64> = starts.iter().map(|&s| m.lo + 4 * s as u64).collect();
        let n = if opts.limit == 0 { abs.len() } else { opts.limit.min(abs.len()) };
        let ends: Vec<u64> = abs[1..].iter().copied().chain([m.hi]).collect();
        let chunks = partition(&est[..n], opts.chunk_fns, opts.chunk_bytes, opts.big_fn);
        let moddir = opts.out.join(&ml.name);
        std::fs::create_dir_all(&moddir)?;
        let idx = modules.len();
        for (number, &(i0, i1)) in chunks.iter().enumerate() {
            let fns = (i0..i1).map(|i| (abs[i], ends[i])).collect();
            tasks.push((
                est[i0..i1].iter().sum(),
                Task { module: idx, number, path: moddir.join(format!("{}_{number:04}.c", ml.name)), fns },
            ));
        }
        progress(&format!(
            "{}: {} functions ({} seeds, {} rounds), {} chunks, discovery {:.1}s",
            ml.name,
            ds.functions,
            ds.seeds,
            ds.rounds,
            chunks.len(),
            t.elapsed().as_secs_f64()
        ));
        emitted.push(abs[..n].to_vec());
        starts_abs.push(abs);
        dstats.push(ds);
        nchunks.push(chunks.len());
        modules.push(m);
    }

    // Largest chunks first, so the pool does not end on a long tail.
    tasks.sort_by_key(|t| -t.0);
    let tasks: Vec<Task> = tasks.into_iter().map(|t| t.1).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<ChunkResult>> = Mutex::new(vec![]);
    let errors: Mutex<Vec<String>> = Mutex::new(vec![]);
    let t_emit = Instant::now();
    let per_module: Vec<(BTreeMap<u64, (usize, Vec<u32>)>, BTreeMap<u64, u64>, BTreeMap<u64, (usize, u64)>)> = modules
        .iter()
        .map(|m| Ok((table.for_module(&m.name, m.base)?, table.spins_for(&m.name, m.base), table.spans_for(&m.name, m.base))))
        .collect::<Result<_>>()?;
    std::thread::scope(|scope| {
        for _ in 0..opts.jobs.max(1) {
            scope.spawn(|| {
                let mut emitters: HashMap<usize, Emitter> = HashMap::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(task) = tasks.get(i) else { break };
                    if !errors.lock().unwrap().is_empty() {
                        break;
                    }
                    let t = Instant::now();
                    let e = emitters.entry(task.module).or_insert_with(|| {
                        let (p, s, sp) = per_module[task.module].clone();
                        Emitter::new(&modules[task.module], &starts_abs[task.module], p, s, sp, &opts.emit)
                    });
                    e.stats.clear();
                    e.unsupported.clear();
                    let name = &modules[task.module].name;
                    let text = match e.chunk_text(name, task.number, &task.fns) {
                        Ok(t) => t,
                        Err(err) => {
                            errors.lock().unwrap().push(format!("{name} chunk {}: {err}", task.number));
                            break;
                        }
                    };
                    if let Err(err) = write_if_changed(&task.path, text.as_bytes()) {
                        errors.lock().unwrap().push(format!("{}: {err}", task.path.display()));
                    }
                    results.lock().unwrap().push(ChunkResult {
                        module: task.module,
                        number: task.number,
                        file: task.path.file_name().unwrap().to_string_lossy().into_owned(),
                        functions: task.fns.len(),
                        bytes: text.len(),
                        seconds: t.elapsed().as_secs_f64(),
                        stats: std::mem::take(&mut e.stats),
                        unsupported: std::mem::take(&mut e.unsupported),
                    });
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if d % 50 == 0 || d == tasks.len() {
                        progress(&format!("C generated: {d}/{}", tasks.len()));
                    }
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(anyhow!("generation failed:\n  {}", errors.join("\n  ")));
    }
    let emit_s = t_emit.elapsed().as_secs_f64();

    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| (r.module, r.number));
    let mut report = GenReport::default();
    for (idx, m) in modules.iter().enumerate() {
        let rs: Vec<&ChunkResult> = results.iter().filter(|r| r.module == idx).collect();
        let mut stats: BTreeMap<&'static str, u64> = BTreeMap::new();
        let mut unsupported: BTreeMap<String, u64> = BTreeMap::new();
        for r in &rs {
            for (k, v) in &r.stats {
                *stats.entry(k).or_default() += v;
            }
            for (k, v) in &r.unsupported {
                *unsupported.entry(k.clone()).or_default() += v;
            }
        }
        let moddir = opts.out.join(&m.name);
        write_if_changed(&moddir.join(format!("{}_table.c", m.name)), table_text(&m.name, &emitted[idx]).as_bytes())?;
        remove_stale(&moddir, &m.name, nchunks[idx])?;
        let c_bytes: usize = rs.iter().map(|r| r.bytes).sum();
        let d = &dstats[idx];
        let mut census: Vec<(&String, &u64)> = unsupported.iter().collect();
        census.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let manifest = json!({
            "module": m.name,
            "marker": GENERATED_MARKER,
            "base": format!("{:#x}", m.base),
            "text": [format!("{:#x}", m.lo), format!("{:#x}", m.hi)],
            "function_count": emitted[idx].len(),
            "build_id": m.nso.build_id,
            "chunks": rs.iter().map(|r| r.file.clone()).collect::<Vec<_>>(),
            "table": format!("{}_table.c", m.name),
            "chunk_info": rs.iter().map(|r| json!({"file": r.file, "functions": r.functions, "bytes": r.bytes,
                                                    "seconds": (r.seconds * 100.0).round() / 100.0})).collect::<Vec<_>>(),
            "stats": {
                "seeds": d.seeds, "bl_targets": d.bl_targets, "materialised_targets": d.materialised_targets,
                "initial_starts": d.initial_starts, "rounds": d.rounds, "functions": d.functions,
                "emitted_functions": stats.get("functions").copied().unwrap_or(0),
                "counters": stats,
                "c_bytes": c_bytes,
                "largest_chunk_bytes": rs.iter().map(|r| r.bytes).max().unwrap_or(0),
            },
            "unsupported_census": census.iter().take(50).map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
            "timings": {"emit_wall_s": (emit_s * 100.0).round() / 100.0},
        });
        write_if_changed(&moddir.join("manifest.json"), (serde_json::to_string_pretty(&manifest)? + "\n").as_bytes())?;
        progress(&format!(
            "{}: {} functions, {} chunks, {:.0} MB of C, {} jump tables, {} all-label fallbacks, {} br via dispatch, {} traps ({} unsupported)",
            m.name,
            stats.get("functions").unwrap_or(&0),
            rs.len(),
            c_bytes as f64 / 1e6,
            stats.get("jump_tables").unwrap_or(&0),
            stats.get("all_label_fallbacks").unwrap_or(&0),
            stats.get("br_dispatch").unwrap_or(&0),
            stats.get("traps").unwrap_or(&0),
            stats.get("unsupported").unwrap_or(&0),
        ));
        report.modules.push(ModuleReport {
            name: m.name.clone(),
            build_id: m.nso.build_id.clone(),
            functions: emitted[idx].len(),
            chunks: rs.len(),
            c_bytes,
            discover: d.clone(),
            stats,
            unsupported,
        });
    }
    report.seconds = t0.elapsed().as_secs_f64();
    Ok(report)
}

/// Deletes chunks an earlier run wrote beyond this run's count, and temporary files an interrupted run left.
fn remove_stale(moddir: &Path, name: &str, nchunks: usize) -> Result<usize> {
    let pat = regex::Regex::new(&format!(r"^{}_(\d{{4}})\.c$", regex::escape(name)))?;
    let mut removed = 0;
    for e in std::fs::read_dir(moddir)? {
        let p = e?.path();
        let f = p.file_name().unwrap().to_string_lossy().into_owned();
        let stale = pat.captures(&f).is_some_and(|c| c[1].parse::<usize>().unwrap() >= nchunks)
            || (f.starts_with('.') && f.ends_with(".tmp"));
        if stale {
            std::fs::remove_file(&p)?;
            removed += 1;
        }
    }
    Ok(removed)
}

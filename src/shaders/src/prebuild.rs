//! The build step: open the game, find every shader, translate them all to SPIR-V in parallel, validate each, and
//! write them with a manifest keyed by the runtime's shader key (the layout is in docs/INTERFACES.md, "shaders").

use crate::job::{self, JobOptions};
use crate::nvn::Stage;
use crate::reflect;
use crate::scan::{self, ScanReport};
use crate::xlate::{self, Tools, Work};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use translator::package::{self, OpenOptions, UpdateChoice};

/// The output layout's version: a change here makes every reader refuse an older folder.
pub const LAYOUT: &str = "nxshaders 1";
/// The reflection's version: part of each output's identity.
const REFLECTION: &str = "reflection 3";

#[derive(Clone)]
pub struct Options {
    pub keys: Option<PathBuf>,
    pub update: UpdateChoice,
    /// Parallel translator processes (and twice as many validators).
    pub jobs: usize,
    /// Threads reading the game's files.
    pub scan_jobs: usize,
    /// Only find the shaders (no translation).
    pub scan_only: bool,
    /// Translate at most this many (a sample, for tests).
    pub limit: Option<usize>,
    pub job: JobOptions,
    /// Jobs per translator process.
    pub batch: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { keys: None, update: UpdateChoice::Embedded, jobs: 4, scan_jobs: 8, scan_only: false, limit: None, job: JobOptions::default(), batch: 64 }
    }
}

/// What a build did.
#[derive(Default, Debug, Clone)]
pub struct Report {
    pub found: usize,
    pub by_stage: BTreeMap<String, usize>,
    pub translated: usize,
    pub reused: usize,
    pub failed: usize,
    pub skipped: usize,
    pub valid: usize,
    pub invalid: usize,
    pub scan_problems: usize,
    pub conflicts: usize,
    pub pairs: usize,
    pub files: usize,
    pub bytes_read: u64,
    pub scan_seconds: f64,
    pub translate_seconds: f64,
    pub validate_seconds: f64,
    pub total_seconds: f64,
    pub chain: String,
}

fn sha_hex(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
        h.update([0u8]);
    }
    h.finalize()[..12].iter().map(|b| format!("{b:02x}")).collect()
}

fn write_atomic(p: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, p)
}

/// The texture or image slot a c2 handle names (combined textures 0.., separate textures 128 + r, images 0..7);
/// -1 when the handle is not in c2's tables.
fn c2_slot(handle: i64, cbuf_slot: i64, image: bool) -> i64 {
    if cbuf_slot >= 0 && (cbuf_slot & 0xffff) != 2 {
        return -1;
    }
    let (w0, w1, ty) = (handle & 0x3fff, (handle >> 14) & 0x3fff, (handle as u32 >> 28) as i64);
    let words: &[i64] = if ty == 0 || ty == 2 { &[w0] } else { &[w0, w1] };
    for &w in words {
        let b = w * 4;
        if b % 8 != 0 {
            continue;
        }
        if image && (0x120..0x120 + 8 * 8).contains(&b) {
            return (b - 0x120) / 8;
        }
        if !image && (0x20..0x168).contains(&b) {
            return (b - 0x20) / 8;
        }
        if !image && (0x168..0x568).contains(&b) {
            return 128 + (b - 0x168) / 8;
        }
    }
    -1
}

/// The reflection the graphics module reads: every descriptor binding with its Vulkan type and what the game binds
/// there, plus the facts that are not in the SPIR-V.
fn reflection(job: &job::Job, chain: &str, job_sha: &str, spv: &[u8], res: &Value, status: &str, why: Option<&str>) -> Value {
    let m = reflect::reflect(spv);
    let mut by_binding: HashMap<i64, (String, Value)> = HashMap::new();
    for (list, kind) in [("cbufs", "cbuf"), ("sbufs", "sbuf"), ("textures", "texture"), ("images", "image")] {
        for e in res.get(list).and_then(Value::as_array).into_iter().flatten() {
            if let Some(b) = e.get("binding").and_then(Value::as_i64) {
                let mut e = e.clone();
                if kind == "texture" || kind == "image" {
                    let h = e.get("handle").and_then(Value::as_i64).unwrap_or(-1);
                    let cs = e.get("cbuf_slot").and_then(Value::as_i64).unwrap_or(-1);
                    e["slot"] = json!(if h >= 0 { c2_slot(h, cs, kind == "image") } else { -1 });
                }
                by_binding.insert(b, (kind.to_string(), e));
            }
        }
    }
    let set = job::stage_set(job.stage) as u64;
    let mut bindings = vec![];
    let mut layout_problems = vec![];
    if let Ok(m) = &m {
        for b in &m.bindings {
            let mut o = json!({"set": b.set, "binding": b.binding, "type": b.kind, "count": b.count, "name": b.name});
            if b.set == 0 && b.binding == 0 && b.name == "support_buffer" {
                o["resource"] = json!({"kind": "support"});
            } else if let Some((kind, e)) = by_binding.get(&(b.binding as i64)).filter(|_| b.set as u64 == set) {
                let mut r = e.clone();
                r["kind"] = json!(kind);
                if let Some(r) = r.as_object_mut() {
                    r.remove("binding");
                }
                o["resource"] = r;
            } else if let Some(slot) = b.name.rsplit_once("_c").and_then(|(_, n)| n.parse::<u32>().ok()).filter(|_| b.kind == "uniform_buffer") {
                // a constant bank the translator reads for itself (c0: NVN's driver segment with the storage buffer
                // table), named <stage>_c<bank>
                o["resource"] = json!({"kind": "cbuf", "slot": slot, "implicit": true});
            } else {
                o["resource"] = json!({"kind": "translator"});
            }
            if !(b.set == 0 && b.binding == 0) && (b.set as u64 != set || b.binding == 0) {
                layout_problems.push(format!("binding {} in set {}, not the stage's set {set}", b.binding, b.set));
            }
            bindings.push(o);
        }
    }
    let mut r = Map::new();
    r.insert("layout".into(), json!(LAYOUT));
    r.insert("key".into(), json!(job.key));
    r.insert("stage".into(), json!(job.stage.name()));
    r.insert("translated_as".into(), json!(job.xlate_stage.name()));
    r.insert("arch".into(), json!(if job.json.get("arch").is_some() { "sm86" } else { "maxwell" }));
    r.insert("status".into(), json!(status));
    if let Some(w) = why {
        r.insert("why".into(), json!(w));
    }
    r.insert("chain".into(), json!(chain));
    r.insert("job_sha".into(), json!(job_sha));
    r.insert("set".into(), json!(set));
    if let Ok(m) = &m {
        r.insert("execution_model".into(), json!(m.execution_model));
        r.insert("spirv_version".into(), json!(format!("{}.{}", m.version.0, m.version.1)));
        r.insert("capabilities".into(), json!(m.capabilities));
        if let Some(ls) = m.local_size {
            r.insert("local_size".into(), json!(ls));
        }
    }
    r.insert("bindings".into(), Value::Array(bindings));
    if !layout_problems.is_empty() {
        r.insert("layout_problems".into(), json!(layout_problems));
    }
    r.insert("c1".into(), job.json.get("c1").cloned().unwrap_or(json!("")));
    for (k, v) in &job.extra {
        r.insert(k.clone(), v.clone());
    }
    for k in ["uses_frag_coord", "frag_out_map", "geometry_stride", "fetch1d", "notes"] {
        if let Some(v) = res.get(k) {
            r.insert(k.into(), v.clone());
        }
    }
    Value::Object(r)
}

/// Build the shaders of `input` (a folder, NSP or NUP; the update applied as `opts.update` says) into `out_dir`.
pub fn prebuild(input: &Path, out_dir: &Path, opts: &Options, say: &(dyn Fn(&str) + Sync)) -> Result<Report, String> {
    let t0 = Instant::now();
    let game = package::open_game(input, &OpenOptions { keys: opts.keys.clone(), update: opts.update.clone() }).map_err(|e| e.to_string())?;
    let rep = scan::scan(&game.exefs.files, &game.romfs, opts.scan_jobs, say);
    build_from_scan(rep, out_dir, opts, say, t0)
}

/// The same for a game already open (`Game::exefs.files`, `Game::romfs`).
pub fn prebuild_game(exefs: &BTreeMap<String, Vec<u8>>, romfs: &package::RomFs, out_dir: &Path, opts: &Options, say: &(dyn Fn(&str) + Sync)) -> Result<Report, String> {
    let t0 = Instant::now();
    let rep = scan::scan(exefs, romfs, opts.scan_jobs, say);
    build_from_scan(rep, out_dir, opts, say, t0)
}

/// The same for a built game's data folder, as the build stages it (`Game::write_data`): reads `<data>/exefs/*`, the
/// RomFS its marker names (`open_data_dir`) and the add-on content installed there (`dlc`), writes `<data>/shaders/`.
pub fn prebuild_data(data: &Path, opts: &Options, say: &(dyn Fn(&str) + Sync)) -> Result<Report, String> {
    let t0 = Instant::now();
    let romfs = package::open_data_dir(data, opts.keys.as_deref()).map_err(|e| e.to_string())?;
    let mut exefs = BTreeMap::new();
    let dir = data.join("exefs");
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        if e.path().is_file() {
            exefs.insert(e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).map_err(|x| format!("{}: {x}", e.path().display()))?);
        }
    }
    // installed add-on content (translator::package::dlc), each title's RomFS read in place
    let mut addons = vec![];
    for (title, ..) in package::dlc::read_manifest(data) {
        match package::dlc::title_romfs(data, title, opts.keys.as_deref()) {
            Ok(r) => addons.push((title, r)),
            Err(e) => say(&format!("shaders: add-on {title:016x} not scanned: {e}")),
        }
    }
    let refs: Vec<(u64, &package::RomFs)> = addons.iter().map(|(t, r)| (*t, r)).collect();
    let rep = scan::scan_with_addons(&exefs, &romfs, &refs, opts.scan_jobs, say);
    build_from_scan(rep, &data.join("shaders"), opts, say, t0)
}

/// The stage of a control section + code area as the scan reads it: NVN2 by its control's stage byte or SPH, NVN by
/// the code area's magic (compute) or its SPH.
pub fn stage_of(ctl: &[u8], code: &[u8]) -> Option<Stage> {
    if crate::nvn::nvn2_ctl(ctl) {
        return crate::nvn::nvn2_stage(ctl, code);
    }
    if code.get(..4) == Some(&crate::nvn::CS_MAGIC[..]) {
        return Some(Stage::Cs);
    }
    crate::nvn::sph_stage(code)
}

/// The live translator: one shader the game set at run time that the build did not find (its key was missing from
/// `<data>/shaders`), translated and validated into `<out_dir>/<key>/` in the build's layout (`spv/<key>.spv` +
/// `.json`, `manifest.tsv`). Returns its key. Blocking; the graphics layer runs it on a thread of its own.
pub fn translate_one(ctl: &[u8], code: &[u8], out_dir: &Path) -> Result<String, String> {
    let stage = stage_of(ctl, code).ok_or("the shader's stage cannot be told")?;
    if let Some(why) = crate::nvn::check(stage, ctl, code) {
        return Err(why.to_string());
    }
    let key = crate::nvn::skey(ctl, code);
    let mut shaders = BTreeMap::new();
    shaders.insert(key.clone(), scan::Shader { key: key.clone(), stage, ctl: ctl.to_vec(), code: code.to_vec(), source: "live (set by the game at run time)".into(), copies: 1 });
    let rep = ScanReport {
        shaders,
        pairs: Default::default(),
        problems: vec![],
        conflicts: vec![],
        files: 0,
        bytes_read: 0,
        seconds: 0.0,
        by_reader: Default::default(),
        zstd_dicts: 0,
        oodle_streams: 0,
    };
    let opts = Options { jobs: 1, ..Default::default() };
    let r = build_from_scan(rep, &out_dir.join(&key), &opts, &|_| {}, Instant::now())?;
    if r.valid == 1 {
        Ok(key)
    } else {
        Err(format!("translated {}, failed {}, invalid {}", r.translated, r.failed, r.invalid))
    }
}

/// Translate and write what a scan found (used by [`prebuild`]).
pub fn build_from_scan(scan: ScanReport, out_dir: &Path, opts: &Options, say: &(dyn Fn(&str) + Sync), t0: Instant) -> Result<Report, String> {
    let mut report = Report {
        found: scan.shaders.len(),
        scan_problems: scan.problems.len(),
        conflicts: scan.conflicts.len(),
        pairs: scan.pairs.len(),
        files: scan.files,
        bytes_read: scan.bytes_read,
        scan_seconds: scan.seconds,
        ..Default::default()
    };
    for s in scan.shaders.values() {
        *report.by_stage.entry(s.stage.name().to_string()).or_default() += 1;
    }
    say(&format!("shaders: {} found in {:.1} s ({} files, {:.0} MB read, {} problems)", report.found, scan.seconds, scan.files, scan.bytes_read as f64 / 1e6, scan.problems.len()));
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let spv_dir = out_dir.join("spv");
    std::fs::create_dir_all(&spv_dir).map_err(|e| io(&spv_dir, e))?;
    let write_lists = |manifest_rows: &[String], report: &Report| -> Result<(), String> {
        let mut pairs = String::from("# vertex (or mesh) key\tpixel key\tkind (var: the same program variation; prog: an agl program's variation)\n");
        for p in &scan.pairs {
            pairs += &format!("{}\t{}\t{}\n", p.first, p.pixel, if p.prog { "prog" } else { "var" });
        }
        write_atomic(&out_dir.join("pairs.tsv"), pairs.as_bytes()).map_err(|e| io(out_dir, e))?;
        let mut probs = String::new();
        for p in scan.problems.iter().chain(&scan.conflicts) {
            probs += p;
            probs += "\n";
        }
        write_atomic(&out_dir.join("problems.txt"), probs.as_bytes()).map_err(|e| io(out_dir, e))?;
        let mut m = format!("# {LAYOUT}\n# chain {}\n# found {} translated {} reused {} failed {} skipped {} valid {} invalid {}\n", report.chain, report.found, report.translated, report.reused, report.failed, report.skipped, report.valid, report.invalid);
        m += "key\tstage\ttranslated_as\tarch\tstatus\tspv_bytes\tsource\n";
        for r in manifest_rows {
            m += r;
        }
        write_atomic(&out_dir.join("manifest.tsv"), m.as_bytes()).map_err(|e| io(out_dir, e))
    };
    if opts.scan_only {
        let rows: Vec<String> = scan.shaders.values().map(|s| format!("{}\t{}\t-\t{}\tfound\t0\t{}\n", s.key, s.stage, if crate::nvn::nvn2_ctl(&s.ctl) { "sm86" } else { "maxwell" }, s.source)).collect();
        report.total_seconds = t0.elapsed().as_secs_f64();
        write_lists(&rows, &report)?;
        return Ok(report);
    }
    let tools = Tools::find()?;
    let chain = tools.identity()?;
    report.chain = chain.clone();
    let work_dir = out_dir.join(".work");
    let _ = std::fs::remove_dir_all(&work_dir);
    let code_dir = work_dir.join("code");
    let xl_dir = work_dir.join("xl");
    std::fs::create_dir_all(&code_dir).map_err(|e| io(&code_dir, e))?;
    std::fs::create_dir_all(&xl_dir).map_err(|e| io(&xl_dir, e))?;

    // jobs; a shader whose output is current (same chain, same job) is reused
    let mut keys: Vec<&scan::Shader> = scan.shaders.values().collect();
    if let Some(n) = opts.limit {
        // a sample spread over the whole key range, every stage represented
        let step = (keys.len() / n.max(1)).max(1);
        keys = keys.into_iter().step_by(step).take(n).collect();
    }
    let mut jobs: HashMap<String, (job::Job, String)> = HashMap::new();
    let mut rows: BTreeMap<String, String> = BTreeMap::new();
    let mut work = vec![];
    let row = |s: &scan::Shader, j: Option<&job::Job>, status: &str, bytes: usize| {
        let arch = if crate::nvn::nvn2_ctl(&s.ctl) { "sm86" } else { "maxwell" };
        format!("{}\t{}\t{}\t{arch}\t{status}\t{bytes}\t{}\n", s.key, s.stage, j.map(|j| j.xlate_stage.name()).unwrap_or("-"), s.source)
    };
    for s in &keys {
        let j = match job::job_for(&s.key, s.stage, &s.ctl, &s.code, opts.job) {
            Ok(j) => j,
            Err(e) => {
                let st = if s.stage == Stage::Rt { "skipped" } else { "failed" };
                if s.stage == Stage::Rt {
                    report.skipped += 1;
                } else {
                    report.failed += 1;
                }
                let _ = std::fs::remove_file(spv_dir.join(format!("{}.spv", s.key)));
                write_atomic(&spv_dir.join(format!("{}.json", s.key)), serde_json::to_string_pretty(&json!({"layout": LAYOUT, "key": s.key, "stage": s.stage.name(), "status": st, "why": e})).unwrap().as_bytes()).map_err(|e| io(&spv_dir, e))?;
                rows.insert(s.key.clone(), row(s, None, st, 0));
                continue;
            }
        };
        let mut jj = j.json.clone();
        jj.remove("code");
        let job_sha = sha_hex(&[chain.as_bytes(), crate::fixes::VERSION.as_bytes(), REFLECTION.as_bytes(), serde_json::to_string(&jj).unwrap().as_bytes(), &j.code]);
        let prev: Option<Value> = std::fs::read(spv_dir.join(format!("{}.json", s.key))).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(p) = prev.filter(|p| p.get("job_sha").and_then(Value::as_str) == Some(&job_sha) && p.get("layout").and_then(Value::as_str) == Some(LAYOUT)) {
            let st = p.get("status").and_then(Value::as_str).unwrap_or("?").to_string();
            let spv_len = std::fs::metadata(spv_dir.join(format!("{}.spv", s.key))).map(|m| m.len() as usize).unwrap_or(0);
            if st == "ok" && spv_len > 0 || st != "ok" {
                report.reused += 1;
                match st.as_str() {
                    "ok" => report.valid += 1,
                    "invalid" => report.invalid += 1,
                    _ => report.failed += 1,
                }
                rows.insert(s.key.clone(), row(s, Some(&j), &st, spv_len));
                continue;
            }
        }
        let cp = code_dir.join(format!("{}.bin", s.key));
        std::fs::write(&cp, &j.code).map_err(|e| io(&cp, e))?;
        let mut js = j.json.clone();
        js.insert("code".into(), json!(cp.to_string_lossy()));
        work.push(Work { key: s.key.clone(), json: Value::Object(js) });
        jobs.insert(s.key.clone(), (j, job_sha));
    }
    let src_of: HashMap<&str, &scan::Shader> = keys.iter().map(|s| (s.key.as_str(), *s)).collect();

    // translate
    let t1 = Instant::now();
    let n = work.len();
    say(&format!("shaders: translating {n} ({} reused) with {} processes", report.reused, opts.jobs));
    let last = std::sync::Mutex::new(Instant::now());
    let outcomes = xlate::translate(&tools, work, &xl_dir, opts.jobs, opts.batch, Duration::from_secs(120), &|d, t| {
        let mut l = last.lock().unwrap();
        if l.elapsed().as_secs_f64() >= 3.0 || d == t {
            *l = Instant::now();
            let el = t1.elapsed().as_secs_f64();
            say(&format!("shaders: translated {d} of {t} ({:.0} a second)", d as f64 / el.max(0.001)));
        }
    });
    report.translate_seconds = t1.elapsed().as_secs_f64();

    // the SPIR-V edits Vulkan needs (fixes.rs)
    let mut applied: HashMap<String, Vec<&'static str>> = HashMap::new();
    for (k, o) in &outcomes {
        if o.is_ok() {
            let p = xl_dir.join(format!("{k}.spv"));
            let Ok(mut b) = std::fs::read(&p) else { continue };
            let mut names = vec![];
            for f in [crate::fixes::storage_format_caps, crate::fixes::size_query_components, crate::fixes::dynamic_offsets] {
                if let Some((fixed, n)) = f(&b) {
                    b = fixed;
                    names.extend(n);
                }
            }
            if !names.is_empty() {
                std::fs::write(&p, &b).map_err(|e| io(&p, e))?;
                applied.insert(k.clone(), names);
            }
        }
    }

    // validate what was written
    let t2 = Instant::now();
    let spvs: Vec<PathBuf> = outcomes.iter().filter(|(_, o)| o.is_ok()).map(|(k, _)| xl_dir.join(format!("{k}.spv"))).collect();
    say(&format!("shaders: validating {} with spirv-val", spvs.len()));
    let val = xlate::validate(&tools, &spvs, opts.jobs * 2, &|d, t| say(&format!("shaders: validated {d} of {t}")));
    report.validate_seconds = t2.elapsed().as_secs_f64();

    // publish: <key>.spv, then <key>.json (a reader takes a shader once its json is there)
    for (key, outcome) in &outcomes {
        let (j, job_sha) = &jobs[key];
        let s = src_of[key.as_str()];
        report.translated += outcome.is_ok() as usize;
        let res: Value = std::fs::read(xl_dir.join(format!("{key}.res.json"))).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
        let (status, why, spv) = match outcome {
            Err(e) => {
                report.failed += 1;
                ("failed", Some(e.clone()), vec![])
            }
            Ok(()) => {
                let spv = std::fs::read(xl_dir.join(format!("{key}.spv"))).unwrap_or_default();
                match val.get(&xl_dir.join(format!("{key}.spv"))).cloned().flatten() {
                    None => {
                        report.valid += 1;
                        ("ok", None, spv)
                    }
                    Some(e) => {
                        report.invalid += 1;
                        ("invalid", Some(e), spv)
                    }
                }
            }
        };
        let mut r = reflection(j, &chain, job_sha, &spv, &res, status, why.as_deref());
        if let Some(f) = applied.get(key) {
            r["fixes"] = json!(f);
        }
        let sp = spv_dir.join(format!("{key}.spv"));
        if spv.is_empty() {
            let _ = std::fs::remove_file(&sp);
        } else {
            write_atomic(&sp, &spv).map_err(|e| io(&sp, e))?;
        }
        let jp = spv_dir.join(format!("{key}.json"));
        write_atomic(&jp, serde_json::to_string_pretty(&r).unwrap().as_bytes()).map_err(|e| io(&jp, e))?;
        rows.insert(key.clone(), row(s, Some(j), status, spv.len()));
    }
    let _ = std::fs::remove_dir_all(&work_dir);
    report.total_seconds = t0.elapsed().as_secs_f64();
    let rows: Vec<String> = rows.into_values().collect();
    write_lists(&rows, &report)?;
    write_atomic(&out_dir.join("LAYOUT"), format!("{LAYOUT}\n").as_bytes()).map_err(|e| io(out_dir, e))?;
    say(&format!(
        "shaders: {} found, {} translated ({} reused), {} failed, {} skipped, {} valid, {} invalid; {:.0} s",
        report.found, report.translated, report.reused, report.failed, report.skipped, report.valid, report.invalid, report.total_seconds
    ));
    Ok(report)
}

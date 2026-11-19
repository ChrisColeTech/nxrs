//! The translator, `dmsh_xlate` (nxnative's tool on Ryujinx's shader translator, built as a native executable), run
//! as several processes at once over batches of jobs; and `spirv-val` over what it writes.
//!
//! `dmsh_xlate <jobs.json> <out dir>` writes `<key>.spv` and `<key>.res.json` per job and prints
//! `[RED] <key>: <reason>` for each it refuses. A batch whose process dies or hangs is split and run again, so one
//! shader that crashes the translator costs only itself.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(windows)]
const BELOW_NORMAL: u32 = 0x0000_4000;
#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

fn command(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(BELOW_NORMAL | NO_WINDOW);
    }
    c
}

/// Where the tools are: `dmsh_xlate.exe` (built from nxnative's source into build/tools/dmsh_xlate) and the Vulkan
/// SDK's `spirv-val.exe`.
#[derive(Clone, Debug)]
pub struct Tools {
    pub xlate: PathBuf,
    pub spirv_val: PathBuf,
}

impl Tools {
    /// `NXSHADERS_XLATE` / `NXSHADERS_SPIRV_VAL`, else beside this executable (`tools/`), else the repository's
    /// build/tools and the Vulkan SDK (`VULKAN_SDK`, else C:\VulkanSDK\1.3.268.0).
    pub fn find() -> Result<Tools, String> {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let mut xl: Vec<PathBuf> = vec![];
        if let Ok(p) = std::env::var("NXSHADERS_XLATE") {
            xl.push(p.into());
        }
        if let Some(d) = &exe_dir {
            xl.push(d.join("tools").join("dmsh_xlate.exe"));
            xl.push(d.join("dmsh_xlate.exe"));
            for up in d.ancestors().take(5) {
                xl.push(up.join("build").join("tools").join("dmsh_xlate").join("dmsh_xlate.exe"));
            }
        }
        xl.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/tools/dmsh_xlate/dmsh_xlate.exe"));
        let xlate = xl.into_iter().find(|p| p.is_file()).ok_or("dmsh_xlate.exe not found (set NXSHADERS_XLATE, or build it: src/shaders/tools/build_dmsh_xlate.ps1)")?;
        let mut sv: Vec<PathBuf> = vec![];
        if let Ok(p) = std::env::var("NXSHADERS_SPIRV_VAL") {
            sv.push(p.into());
        }
        if let Ok(p) = std::env::var("VULKAN_SDK") {
            sv.push(Path::new(&p).join("Bin").join("spirv-val.exe"));
        }
        sv.push(PathBuf::from(r"C:\VulkanSDK\1.3.268.0\Bin\spirv-val.exe"));
        let spirv_val = sv.into_iter().find(|p| p.is_file()).ok_or("spirv-val.exe not found (set NXSHADERS_SPIRV_VAL or VULKAN_SDK)")?;
        Ok(Tools { xlate: dunce(xlate), spirv_val })
    }

    /// The translator's identity: its source hash (embedded at build) and the executable's own hash.
    pub fn identity(&self) -> Result<String, String> {
        let out = command(&self.xlate).arg("--source-sha").output().map_err(|e| format!("{}: {e}", self.xlate.display()))?;
        let src = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let bin = std::fs::read(&self.xlate).map_err(|e| e.to_string())?;
        let h = Sha256::digest(&bin);
        Ok(format!("dmsh_xlate {src} bin {}", h[..8].iter().map(|b| format!("{b:02x}")).collect::<String>()))
    }
}

fn dunce(p: PathBuf) -> PathBuf {
    std::fs::canonicalize(&p).map(|c| PathBuf::from(c.to_string_lossy().trim_start_matches(r"\\?\"))).unwrap_or(p)
}

/// One translation: the job (its code file already written) and where the output goes.
pub struct Work {
    pub key: String,
    pub json: serde_json::Value,
}

/// A translation's result: Ok(()) when `<key>.spv` was written, else the reason.
pub type Outcome = Result<(), String>;

/// Translate `work` in batches over `jobs` processes into `out`. `progress(done, total)` is called as batches end.
pub fn translate(tools: &Tools, work: Vec<Work>, out: &Path, jobs: usize, batch: usize, timeout: Duration, progress: &(dyn Fn(usize, usize) + Sync)) -> HashMap<String, Outcome> {
    let total = work.len();
    let queue: Mutex<Vec<Vec<Work>>> = Mutex::new({
        let mut q = vec![];
        let mut it = work.into_iter().peekable();
        while it.peek().is_some() {
            q.push(it.by_ref().take(batch.max(1)).collect::<Vec<_>>());
        }
        q.reverse();
        q
    });
    let results: Mutex<HashMap<String, Outcome>> = Mutex::new(HashMap::new());
    let done = AtomicUsize::new(0);
    let seq = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| loop {
                let Some(b) = queue.lock().unwrap().pop() else { break };
                let n = seq.fetch_add(1, Ordering::Relaxed);
                let res = run_batch(tools, &b, out, n, timeout);
                match res {
                    BatchEnd::Finished(errs) => {
                        let mut r = results.lock().unwrap();
                        for w in &b {
                            let ok = out.join(format!("{}.spv", w.key)).is_file();
                            let o = match errs.get(&w.key) {
                                Some(e) => Err(e.clone()),
                                None if ok => Ok(()),
                                None => Err("the translator wrote no SPIR-V and gave no reason".into()),
                            };
                            r.insert(w.key.clone(), o);
                        }
                        drop(r);
                        let d = done.fetch_add(b.len(), Ordering::Relaxed) + b.len();
                        progress(d, total);
                    }
                    BatchEnd::Died(why) => {
                        if b.len() == 1 {
                            results.lock().unwrap().insert(b[0].key.clone(), Err(format!("the translator {why}")));
                            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                            progress(d, total);
                        } else {
                            // split and run the halves again; the finished keys are kept as their files show
                            let mut b = b;
                            let rest = b.split_off(b.len() / 2);
                            let mut q = queue.lock().unwrap();
                            q.push(b);
                            q.push(rest);
                        }
                    }
                }
            });
        }
    });
    results.into_inner().unwrap()
}

enum BatchEnd {
    Finished(HashMap<String, String>),
    Died(String),
}

fn run_batch(tools: &Tools, b: &[Work], out: &Path, n: usize, timeout: Duration) -> BatchEnd {
    let jp = out.join(format!("job_{n}_{}.json", std::process::id()));
    let arr = serde_json::Value::Array(b.iter().map(|w| w.json.clone()).collect());
    if let Err(e) = std::fs::write(&jp, serde_json::to_vec(&arr).unwrap()) {
        return BatchEnd::Died(format!("could not be given its jobs ({e})"));
    }
    let errf = out.join(format!("job_{n}_{}.err", std::process::id()));
    let child = command(&tools.xlate)
        .arg(&jp)
        .arg(out)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&errf).map(Stdio::from).unwrap_or(Stdio::null()))
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return BatchEnd::Died(format!("did not start ({e})")),
    };
    let t0 = Instant::now();
    let limit = timeout + Duration::from_secs(2) * b.len() as u32;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if t0.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let stderr = std::fs::read_to_string(&errf).unwrap_or_default();
    let _ = std::fs::remove_file(&jp);
    let _ = std::fs::remove_file(&errf);
    let mut errs = HashMap::new();
    for ln in stderr.lines() {
        if let Some(rest) = ln.strip_prefix("[RED] ") {
            if let Some((k, why)) = rest.split_once(": ") {
                errs.insert(k.to_string(), why.chars().take(300).collect::<String>());
            }
        }
    }
    match status {
        None => BatchEnd::Died(format!("ran past {}s and was stopped", limit.as_secs())),
        // exit 1 = some jobs refused, each with its [RED] line; anything else is a crash
        Some(s) if s.success() || s.code() == Some(1) => BatchEnd::Finished(errs),
        Some(s) => {
            let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
            BatchEnd::Died(format!("exited with {s} ({})", tail.chars().take(200).collect::<String>()))
        }
    }
}

/// `spirv-val --target-env vulkan1.3` over each file, `jobs` at a time → path → the validator's first message (None
/// when valid).
pub fn validate(tools: &Tools, files: &[PathBuf], jobs: usize, progress: &(dyn Fn(usize, usize) + Sync)) -> HashMap<PathBuf, Option<String>> {
    let next = AtomicUsize::new(0);
    let res: Mutex<HashMap<PathBuf, Option<String>>> = Mutex::new(HashMap::new());
    let done = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(f) = files.get(i) else { break };
                let r = command(&tools.spirv_val).args(["--target-env", "vulkan1.3"]).arg(f).output();
                let v = match r {
                    Ok(o) if o.status.success() => None,
                    Ok(o) => {
                        let msg = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
                        Some(msg.lines().find(|l| l.contains("error")).unwrap_or(msg.lines().next().unwrap_or("invalid")).trim().chars().take(300).collect())
                    }
                    Err(e) => Some(format!("spirv-val did not run: {e}")),
                };
                res.lock().unwrap().insert(f.clone(), v);
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                if d % 500 == 0 || d == files.len() {
                    progress(d, files.len());
                }
            });
        }
    });
    res.into_inner().unwrap()
}

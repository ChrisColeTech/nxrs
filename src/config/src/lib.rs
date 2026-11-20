//! nxconfig: the game's settings, `<name>.ini` beside the executable, with every option Turbo's runtime reads, and the
//! engine-wide registries other parts publish into (control commands, counters).
//!
//! The front end opens the ini (`open`), applies `--key=value` arguments for one run (`apply_arg`), and polls the file
//! (`poll`) so an edit applies live. Every other part reads values here (`f32`, `int`, `string`, `flag` and the
//! helpers) and may watch `generation()` to notice a change. Values are already normalised to Turbo's ranges.
//! The file is rewritten sorted into sections with a comment above each key; keys the table does not know are kept
//! under `[Other]`.

pub mod ctl;
pub mod metrics;
pub mod options;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

/// The current settings version (Turbo's `settings_version`): 2 moved docked to 1, 3 moved game_dynamic_resolution
/// to auto, 4 moved lsfg from the old default 1 to 0 (frame generation off by default, one global setting:
/// docs/DECISIONS.md 1). An ini below a version gets that change once.
pub const SETTINGS_VERSION: u32 = 4;

struct Store {
    /// Values from the file (and changes made in the game), as written.
    file: BTreeMap<String, String>,
    /// `--key=value` for this run: read, never written.
    run: BTreeMap<String, String>,
}

static STORE: RwLock<Store> = RwLock::new(Store { file: BTreeMap::new(), run: BTreeMap::new() });
static GEN: AtomicU64 = AtomicU64::new(1);

struct FileState {
    path: Option<PathBuf>,
    title: String,
    mtime: Option<SystemTime>,
    dirty: bool,
}
static FILE: Mutex<FileState> = Mutex::new(FileState { path: None, title: String::new(), mtime: None, dirty: false });

type Listener = Box<dyn Fn(&str, &str) + Send + Sync>;
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

/// Called with (key, value) for each key whose value changed (the file reloaded, a hotkey, `set`).
pub fn on_change(f: impl Fn(&str, &str) + Send + Sync + 'static) {
    LISTENERS.lock().unwrap().push(Box::new(f));
}

/// Bumped whenever any value changes.
pub fn generation() -> u64 {
    GEN.load(Ordering::Acquire)
}

/// The value of `key` (normalised), else the table's default, else "".
pub fn string(key: &str) -> String {
    let s = STORE.read().unwrap();
    if let Some(v) = s.run.get(key).or_else(|| s.file.get(key)) {
        return v.clone();
    }
    options::find(key).map(|o| o.default.to_string()).unwrap_or_default()
}

/// Whether the key has a value from the file or the command line (not just the default).
pub fn is_set(key: &str) -> bool {
    let s = STORE.read().unwrap();
    s.run.contains_key(key) || s.file.contains_key(key)
}

pub fn f32(key: &str) -> f32 {
    string(key).trim().parse().unwrap_or_else(|_| options::find(key).and_then(|o| o.default.parse().ok()).unwrap_or(0.0))
}

/// The leading integer (C's atoi, as Turbo reads its ints).
pub fn int(key: &str) -> i64 {
    let v = string(key);
    let t = v.trim();
    let (neg, d) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t),
    };
    let n: i64 = d.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
    if neg {
        -n
    } else {
        n
    }
}

pub fn flag(key: &str) -> bool {
    int(key) != 0
}

/// "auto" → None, else Some(value != 0) (game_dynamic_resolution, driver_exclusive, shader_process).
pub fn tristate(key: &str) -> Option<bool> {
    let v = string(key);
    if v.trim() == "auto" {
        None
    } else {
        Some(int(key) != 0)
    }
}

/// The display cap in images a second: 0 unlimited, 30, or 60..240 (`fps_limit`: 1 and anything below 60 but 30 and
/// 0 mean 60).
pub fn fps_limit_hz() -> u32 {
    let n = int("fps_limit");
    match n {
        0 => 0,
        30 => 30,
        60..=240 => n as u32,
        _ => 60,
    }
}

/// `fps_target`: 30 / 20 draw every 2nd / 3rd frame (1 / 2 frames skipped), 120 turns frame generation x2 on.
pub fn frame_skip() -> u32 {
    match int("fps_target") {
        30 => 1,
        20 => 2,
        _ => 0,
    }
}

/// Frame generation wanted, its multiplier and target: `lsfg` + `lsfg_multiplier` + `lsfg_target`, with
/// `fps_target=120` forcing x2 at 120 (Turbo's rule).
pub fn lsfg() -> (bool, u32, f32) {
    if int("fps_target") == 120 {
        return (true, 2, 120.0);
    }
    (flag("lsfg"), int("lsfg_multiplier").clamp(2, 4) as u32, f32("lsfg_target"))
}

fn changed(key: &str, value: &str) {
    GEN.fetch_add(1, Ordering::AcqRel);
    for l in LISTENERS.lock().unwrap().iter() {
        l(key, value);
    }
}

/// Stores a value from the file or the game (written back to the file). Returns false for a removed key.
fn store_file(key: &str, value: &str) -> bool {
    let k = options::canonical(key.trim());
    let Some(v) = options::normalise(k, value) else { return false };
    let old = {
        let mut s = STORE.write().unwrap();
        s.file.insert(k.to_string(), v.clone())
    };
    if old.as_deref() != Some(v.as_str()) {
        changed(k, &v);
    }
    true
}

/// A change made while playing (F4, F11, the F1 page): kept and written to the file at the next `poll`.
pub fn set(key: &str, value: &str) {
    if store_file(key, value) {
        FILE.lock().unwrap().dirty = true;
    }
}

/// `--key=value` (or `key=value`): any option for this run only; the file is not written with it. Returns false when
/// the argument is not of that form.
pub fn apply_arg(arg: &str) -> bool {
    let a = arg.strip_prefix("--").unwrap_or(arg);
    let Some((k, v)) = a.split_once('=') else { return false };
    let k = options::canonical(k.trim()).to_string();
    let Some(v) = options::normalise(&k, v) else { return true };
    STORE.write().unwrap().run.insert(k.clone(), v.clone());
    changed(&k, &v);
    true
}

/// Parses ini text into (key, value) pairs, in order: `key=value` lines, `;` / `#` comments and `[sections]` skipped,
/// whitespace around keys and values trimmed.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for line in text.lines() {
        let l = line.trim_start();
        if l.starts_with(';') || l.starts_with('#') || l.starts_with('[') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                out.push((k.to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

/// The file's text for the current values: a header, each section with each key's comment, then `[Other]`.
pub fn render(title: &str) -> String {
    let s = STORE.read().unwrap();
    let mut o = String::new();
    o.push_str("; =====================================================================\n");
    o.push_str(&format!(";  {title} - settings\n"));
    o.push_str(";  Edit a value and save: most settings apply live while the game runs\n");
    o.push_str(";  (docked, language, fullscreen and frame generation apply on restart).\n");
    o.push_str(";  Keys in game: F1 help, F3 screenshot, F4 FPS limit, F5 trace,\n");
    o.push_str(";  F7 record video, F8 full-res screenshot, F11 fullscreen, Esc quit.\n");
    o.push_str("; =====================================================================\n");
    for sec in options::SECTIONS {
        o.push_str(&format!("\n[{}]\n", sec.title));
        for opt in sec.opts {
            let v = s.file.get(opt.key).map(|v| v.as_str()).unwrap_or(opt.default);
            o.push_str(&format!("; {}\n{}={}\n", opt.help, opt.key, v));
        }
    }
    let mut other = false;
    for (k, v) in &s.file {
        if options::find(k).is_some() {
            continue;
        }
        if !other {
            o.push_str("\n[Other]\n");
            other = true;
        }
        o.push_str(&format!("{k}={v}\n"));
    }
    o
}

fn load_text(text: &str) {
    for (k, v) in parse(text) {
        store_file(&k, &v);
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn write_file(f: &mut FileState) -> std::io::Result<()> {
    let Some(p) = f.path.clone() else { return Ok(()) };
    let tmp = p.with_extension("ini.tmp");
    std::fs::write(&tmp, render(&f.title).replace('\n', "\r\n"))?;
    // a rename over the file: a reader never sees half of it
    let _ = std::fs::remove_file(&p);
    std::fs::rename(&tmp, &p)?;
    f.mtime = mtime(&p);
    f.dirty = false;
    Ok(())
}

/// Opens the game's ini: reads it (a missing file is all defaults), applies the settings-version changes once, and
/// writes it back complete, with comments. `title` heads the file.
pub fn open(path: &Path, title: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    load_text(&text);
    let v = int("settings_version");
    if v < 2 {
        store_file("docked", "1");
    }
    if v < 3 {
        store_file("game_dynamic_resolution", "auto");
    }
    // the old default (1) only: a player's own 0 stays (an explicit 1 written before can not be told from it)
    if v < 4 && string("lsfg").trim() == "1" {
        store_file("lsfg", "0");
    }
    if v < SETTINGS_VERSION as i64 {
        store_file("settings_version", &SETTINGS_VERSION.to_string());
    }
    let mut f = FILE.lock().unwrap();
    f.path = Some(path.to_path_buf());
    f.title = title.to_string();
    write_file(&mut f)
}

/// The ini's path, once opened.
pub fn path() -> Option<PathBuf> {
    FILE.lock().unwrap().path.clone()
}

/// Once a second from the front end: writes changes made in the game, or reloads the file when it was edited (and
/// writes it back normalised). Returns true when it reloaded.
pub fn poll() -> bool {
    let mut f = FILE.lock().unwrap();
    let Some(p) = f.path.clone() else { return false };
    if f.dirty {
        let _ = write_file(&mut f);
        return false;
    }
    let m = mtime(&p);
    if m.is_none() || m == f.mtime {
        return false;
    }
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let keys: Vec<(String, String)> = parse(&text);
    {
        // a key deleted from the file goes back to its default
        let present: std::collections::HashSet<String> = keys.iter().map(|(k, _)| options::canonical(k).to_string()).collect();
        let gone: Vec<String> = STORE.read().unwrap().file.keys().filter(|k| !present.contains(*k)).cloned().collect();
        for k in gone {
            STORE.write().unwrap().file.remove(&k);
            changed(&k, &string(&k));
        }
    }
    for (k, v) in keys {
        store_file(&k, &v);
    }
    let _ = write_file(&mut f);
    true
}

/// Writes pending changes now (at exit).
pub fn flush() {
    let mut f = FILE.lock().unwrap();
    if f.dirty {
        let _ = write_file(&mut f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip_with_migration_and_aliases() {
        let d = std::env::temp_dir().join(format!("nxconfig-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("g.ini");
        std::fs::write(&p, "[Display]\nmodo_tv=0\nidioma = 5\nvsync=1\nfps_limit=45\nmy_key=hello\n").unwrap();
        open(&p, "Game").unwrap();
        assert_eq!(string("docked"), "1", "settings_version < 2 moves docked to 1 once");
        assert_eq!(int("language"), 5, "the Spanish alias");
        assert_eq!(fps_limit_hz(), 60);
        assert_eq!(string("game_dynamic_resolution"), "auto");
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("\r\n; Game language"), "comments above the keys");
        assert!(text.contains("[Other]\r\nmy_key=hello"), "unknown keys kept");
        assert!(text.contains("settings_version=4"));
        assert!(!text.contains("vsync=1"), "a removed key is dropped");
        assert!(!text.contains("modo_tv"), "aliases are written under the current name");
        // a game-side change
        set("fps_limit", "0");
        poll();
        assert!(std::fs::read_to_string(&p).unwrap().contains("fps_limit=0"));
        assert_eq!(fps_limit_hz(), 0);
        // per-run argument
        assert!(apply_arg("--resolution=2"));
        assert_eq!(f32("resolution"), 2.0);
        assert!(!std::fs::read_to_string(&p).unwrap().contains("resolution=2"));
        assert!(apply_arg("--fps_target=120"));
        assert_eq!(lsfg(), (true, 2, 120.0));
        assert!(apply_arg("--fps_target=0"));
        // lsfg: the old default 1 moves to 0 once (frame generation off by default); a later 1 stays
        let p = d.join("g2.ini");
        std::fs::write(&p, "settings_version=3\nlsfg=1\n").unwrap();
        open(&p, "Game").unwrap();
        assert_eq!(string("lsfg"), "0", "the old default 1 becomes 0 once");
        assert!(!lsfg().0);
        std::fs::write(&p, "settings_version=4\nlsfg=1\n").unwrap();
        open(&p, "Game").unwrap();
        assert_eq!(string("lsfg"), "1", "a player's 1 after the migration stays");
        assert!(lsfg().0);
        let _ = std::fs::remove_dir_all(&d);
    }
}

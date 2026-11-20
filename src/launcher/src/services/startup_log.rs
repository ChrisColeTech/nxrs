//! A few lines in `%TEMP%\nxrs-startup.log` (the system temp folder elsewhere) as the launcher starts:
//! each step's name and the paths it uses, never the library's contents. A double-clicked program has
//! no console, so this is where a start that ends early says how far it got. Each start begins a new
//! file.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

fn path() -> std::path::PathBuf {
    std::env::temp_dir().join("nxrs-startup.log")
}

/// One step: appended with the time; the first of a run truncates the file.
pub fn note(step: &str) {
    let first = !STARTED.swap(true, Ordering::SeqCst);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!first)
        .truncate(first)
        .open(path());
    if let Ok(mut f) = file {
        let now = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(f, "{now} {step}");
    }
}

//! Engine runs off the UI thread: one child process, its stdout and stderr merged into one pipe, read
//! line by line on a worker thread (appended raw to a log file when there is one); its lines and its
//! end arrive over a channel.
mod job;
mod reader;

pub use job::{Job, Spec};

use std::sync::Arc;

/// Wakes the UI when something arrived.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    /// one output line, trimmed (machine `@` lines included: the caller decides what to show)
    Line(String),
    /// the process ended: its exit code (None when it had none)
    Exited(Option<i32>),
}

/// Nobody to wake (headless runs, tests).
pub fn noop() -> Notify {
    Arc::new(|| {})
}

/// Creates (truncates) a build log with its header line.
pub fn open_build_log(path: &str) -> Result<std::fs::File, String> {
    use std::io::Write;
    let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let stamp = crate::model::dates::format(&Some(crate::model::dates::now()));
    let header = format!("{}{stamp}\n", crate::constants::words_build::LOG_HEADER);
    f.write_all(header.as_bytes()).map_err(|e| e.to_string())?;
    Ok(f)
}

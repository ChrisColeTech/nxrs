//! One engine run: started, drained a bounded number of events at a time, cancelled with everything
//! it started.

use super::reader::{SharedLog, read_lines};
use super::{JobEvent, Notify};
use crate::constants::timing;
use crate::services::platform::{self, ProcessTree};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Spec<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub env: &'a [(String, String)],
    /// the build log, created with its header; the raw output is appended
    pub log: Option<std::fs::File>,
}

pub struct Job {
    rx: Receiver<JobEvent>,
    tree: Arc<ProcessTree>,
    cancelled: AtomicBool,
    log: Option<SharedLog>,
}

impl Job {
    pub fn start(spec: Spec, notify: Notify) -> Result<Job, String> {
        let (reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(spec.program);
        let out = writer.try_clone().map_err(|e| e.to_string())?;
        cmd.args(spec.args)
            .current_dir(spec.cwd)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(writer);
        cmd.envs(spec.env.iter().map(|(k, v)| (k, v)));
        cmd.env("PYTHONUTF8", "1").env("PYTHONUNBUFFERED", "1");
        platform::prepare_tree(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        // the parent's write ends go: the reader sees the end when the children end
        drop(cmd);
        let tree = Arc::new(ProcessTree::adopt(&child));
        let (tx, rx) = channel();
        let log = spec.log.map(|f| Arc::new(Mutex::new(f)));
        let (done_tx, done_rx) = channel::<()>();
        let (reader_tx, reader_notify, reader_log) = (tx.clone(), notify.clone(), log.clone());
        std::thread::spawn(move || {
            read_lines(reader, reader_tx, reader_notify, reader_log, done_tx)
        });
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code());
            // what is left is drained; a grandchild that still holds the pipe does not keep the job
            let _ = done_rx.recv_timeout(Duration::from_millis(timing::DRAIN_MS));
            let _ = tx.send(JobEvent::Exited(code));
            notify();
        });
        Ok(Job {
            rx,
            tree,
            cancelled: AtomicBool::new(false),
            log,
        })
    }

    /// Up to `max` events that arrived.
    pub fn drain(&mut self, max: usize) -> Vec<JobEvent> {
        self.rx.try_iter().take(max).collect()
    }
    /// Waits for the next event (headless runs, tests).
    pub fn wait_event(&mut self, timeout: Duration) -> Option<JobEvent> {
        self.rx.recv_timeout(timeout).ok()
    }
    /// Ends the run and everything it started (Linux: SIGTERM to the group, SIGKILL after 4 s).
    pub fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            self.tree
                .terminate(Duration::from_millis(timing::KILL_GRACE_MS));
        }
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    /// Appends a line to the log (the final detail).
    pub fn log_line(&self, text: &str) {
        if let Some(Ok(mut f)) = self.log.as_ref().map(|l| l.lock()) {
            let _ = f.write_all(format!("{text}\n").as_bytes());
            let _ = f.flush();
        }
    }
}

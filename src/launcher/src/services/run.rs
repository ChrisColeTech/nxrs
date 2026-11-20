//! Short runs of nxrs-build that are waited for: `contract`, `doctor`.

use super::platform;
use super::toolchain::Toolchain;
use crate::constants::{timing, words_build as w};
use crate::model::doctor::{self, NxrsDoctor};
use crate::model::path_text::parent;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs `program` to its end or `timeout` (then kills it), stdout and stderr merged: (exit code,
/// output); None when it did not start or finish.
pub fn run_captured(
    program: &str,
    args: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Option<(i32, String)> {
    let (reader, writer) = std::io::pipe().ok()?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(parent(program))
        .stdin(Stdio::null())
        .stdout(writer.try_clone().ok()?)
        .stderr(writer);
    cmd.envs(env.iter().map(|(k, v)| (k, v)));
    platform::no_console(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    drop(cmd);
    let collector = std::thread::spawn(move || {
        let mut r = reader;
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Some((
                    status.code().unwrap_or(-1),
                    collector.join().unwrap_or_default(),
                ));
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// `nxrs-build doctor`, waited for.
pub fn doctor(tools: &Toolchain) -> NxrsDoctor {
    if !super::files::is_file(&tools.nxrs_build) {
        return NxrsDoctor::failed_to_run(w::DOCTOR_MISSING);
    }
    let limit = Duration::from_millis(timing::DOCTOR_MS);
    match run_captured(
        &tools.nxrs_build,
        &["doctor".into()],
        &tools.environment(),
        limit,
    ) {
        Some((code, out)) => doctor::parse(&out, code),
        None => NxrsDoctor::failed_to_run(w::DOCTOR_LATE),
    }
}

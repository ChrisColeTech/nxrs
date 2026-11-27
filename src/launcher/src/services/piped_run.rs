//! A short run of a program fed on stdin: the web server's shader translations (`web_xlate`).

use super::platform;
use crate::model::path_text::parent;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What a finished run said: its exit code, stdout, stderr.
pub struct Ran {
    pub code: Option<i32>,
    pub out: String,
    pub err: String,
}

fn collect(p: Option<impl Read + Send + 'static>) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut p) = p {
            let _ = p.read_to_string(&mut s);
        }
        s
    })
}

/// Runs `program args` from its own folder with `input` on stdin, to its end; None when it did
/// not start (Err: why) or did not end within `limit` (then it is stopped: Ok(None)).
pub fn run(
    program: &str,
    args: &[&std::ffi::OsStr],
    input: &[u8],
    limit: Duration,
) -> Result<Option<Ran>, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(parent(program))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    platform::no_console(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let feed = child.stdin.take().map(|mut i| {
        let body = input.to_vec();
        std::thread::spawn(move || i.write_all(&body))
    });
    let (out, err) = (collect(child.stdout.take()), collect(child.stderr.take()));
    let start = Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
        }
    };
    if let Some(t) = feed {
        let _ = t.join();
    }
    Ok(Some(Ran {
        code,
        out: out.join().unwrap_or_default(),
        err: err.join().unwrap_or_default(),
    }))
}

//! The reader thread of a run: raw bytes to the log, trimmed UTF-8 lines to the channel.

use super::{JobEvent, Notify};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub type SharedLog = Arc<Mutex<std::fs::File>>;

fn append(log: &Option<SharedLog>, bytes: &[u8]) {
    if let Some(Ok(mut f)) = log.as_ref().map(|l| l.lock()) {
        let _ = f.write_all(bytes);
    }
}

pub fn read_lines(
    mut reader: std::io::PipeReader,
    tx: Sender<JobEvent>,
    notify: Notify,
    log: Option<SharedLog>,
    done: Sender<()>,
) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = buf.get(..n).unwrap_or_default();
        append(&log, chunk);
        pending.extend_from_slice(chunk);
        let mut sent = false;
        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(pending.get(..pos).unwrap_or_default())
                .trim()
                .to_string();
            pending.drain(..=pos);
            let _ = tx.send(JobEvent::Line(line));
            sent = true;
        }
        if sent {
            notify();
        }
    }
    if !pending.is_empty() {
        let _ = tx.send(JobEvent::Line(
            String::from_utf8_lossy(&pending).trim().to_string(),
        ));
    }
    if let Some(Ok(mut f)) = log.as_ref().map(|l| l.lock()) {
        let _ = f.flush();
    }
    let _ = done.send(());
    notify();
}

//! The web server's pad bridge (WEB-PADBRIDGE): a page's `GET /<slug>/pad-bridge` WebSocket is
//! answered `101` and the connection is the bridge's from then on. It starts the mount's
//! `nxrs-build pad-bridge` (fixed arguments, its folder as working directory, its stdout and
//! stderr into `<web build>\pad-bridge.log`) and writes each pad line the page sends to its stdin;
//! the engine's native input layer drives the pads' rumble and lights with them. A close, a broken
//! socket or `PAD_IDLE_SECONDS` without a frame ends it: stdin closes (the bridge stops the motors,
//! turns the lights off and exits), and a bridge still running after `PAD_EXIT_MS` is killed.

use super::platform;
use super::web_serve::{Mount, Mounts};
use crate::constants::{web as c, words_web as w};
use crate::model::fill::fill;
use crate::model::path_text::parent;
use crate::model::{http_range, ws};
use std::fs::File;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

/// Whether a request is this route's: a WebSocket upgrade of `/<slug>/pad-bridge`.
pub fn wants(method: &str, path: &str, upgrade: bool) -> bool {
    method == "GET" && upgrade && ws::pad_route(path).is_some()
}

/// Serves the route on this connection until the page goes; nothing else is served on it.
pub fn serve(r: &mut impl Read, out: &mut TcpStream, path: &str, key: Option<&str>, m: &Mounts) {
    let slug = ws::pad_route(path).unwrap_or_default();
    let mount: Option<Mount> = m.lock().ok().and_then(|m| m.get(&slug).cloned());
    let (Some(mount), Some(key)) = (mount, key) else {
        let status = if key.is_none() { 400 } else { 404 };
        let _ = refuse(out, status, w::BAD_PAD_REQUEST);
        return;
    };
    if mount.translator.is_empty() {
        let _ = refuse(out, 503, w::NO_PAD_BRIDGE);
        return;
    }
    let log = mount.root.join(c::PAD_LOG);
    let mut child = match start(&mount.translator, &log) {
        Ok(child) => child,
        Err(e) => {
            note(&slug, &fill(w::PAD_NOT_STARTED, &[&e]));
            let _ = refuse(out, 503, &e);
            return;
        }
    };
    note(&slug, &fill(w::PAD_STARTED, &[&log.to_string_lossy()]));
    let accept = ws::accept(key);
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    let mut lines = 0u64;
    if out.write_all(head.as_bytes()).is_ok() {
        let _ = out.set_read_timeout(Some(Duration::from_secs(c::PAD_IDLE_SECONDS)));
        if let Some(stdin) = child.stdin.take() {
            lines = forward(r, out, stdin);
        }
    }
    drop(child.stdin.take());
    note(&slug, &finish(&mut child, lines));
}

fn start(program: &str, log: &std::path::Path) -> Result<Child, String> {
    let file = File::create(log).map_err(|e| e.to_string())?;
    let err = file.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(program);
    cmd.arg(c::PAD_COMMAND)
        .current_dir(parent(program))
        .stdin(Stdio::piped())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err));
    platform::no_console(&mut cmd);
    cmd.spawn().map_err(|e| e.to_string())
}

/// Frames to the bridge until the socket ends; how many pad lines went through.
fn forward(r: &mut impl Read, out: &mut impl Write, mut stdin: ChildStdin) -> u64 {
    let mut lines = 0;
    while let Some((opcode, payload)) = read_frame(r) {
        match opcode {
            ws::TEXT => {
                let text = String::from_utf8_lossy(&payload);
                if ws::pad_line_ok(&text) {
                    if writeln!(stdin, "{}", text.trim()).is_err() || stdin.flush().is_err() {
                        break; // the bridge ended
                    }
                    lines += 1;
                }
            }
            ws::PING => {
                if out.write_all(&ws::frame(ws::PONG, &payload)).is_err() {
                    break;
                }
            }
            ws::PONG => {}
            _ => {
                let _ = out.write_all(&ws::frame(ws::CLOSE, &[]));
                break; // close, or a frame the bridge never takes
            }
        }
    }
    lines
}

/// One whole client frame: (opcode, unmasked payload); None at the socket's end, on a timeout,
/// an unmasked or fragmented frame, or one larger than `PAD_MAX_FRAME`.
fn read_frame(r: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b).ok()?;
    let h = ws::head(b);
    if !h.fin || !h.masked {
        return None;
    }
    let len = match h.len7 {
        126 => {
            let mut x = [0u8; 2];
            r.read_exact(&mut x).ok()?;
            u16::from_be_bytes(x) as u64
        }
        127 => return None,
        n => n as u64,
    };
    if len > c::PAD_MAX_FRAME {
        return None;
    }
    let mut key = [0u8; 4];
    r.read_exact(&mut key).ok()?;
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).ok()?;
    ws::unmask(&mut payload, key);
    Some((h.opcode, payload))
}

/// Waits for the bridge to end after its stdin closed, else kills it; the line said.
fn finish(child: &mut Child, lines: u64) -> String {
    let t0 = Instant::now();
    let limit = Duration::from_millis(c::PAD_EXIT_MS);
    while t0.elapsed() < limit {
        if !matches!(child.try_wait(), Ok(None)) {
            return fill(w::PAD_ENDED, &[&lines.to_string()]);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    fill(
        w::PAD_KILLED,
        &[&c::PAD_EXIT_MS.to_string(), &lines.to_string()],
    )
}

/// The server's own record (`--serve-web` prints it; a window has no console and loses nothing).
fn note(slug: &str, what: &str) {
    let _ = writeln!(std::io::stdout(), "{}", fill(w::PAD_LINE, &[slug, what]));
}

fn refuse(out: &mut impl Write, status: u16, why: &str) -> std::io::Result<()> {
    let h = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: {}\r\n\
         Connection: close\r\nContent-Length: {}\r\n\r\n{why}",
        http_range::reason(status),
        c::CACHE_NONE,
        why.len()
    );
    out.write_all(h.as_bytes())
}

//! One connection of the web server: requests read until the browser closes it (keep-alive), each
//! answered with the cross-origin isolation headers, `Accept-Ranges`, an exact `206` and
//! `Content-Range` for a range, `416` past the end. GET and HEAD only.

use super::web_serve::{Mount, Mounts};
use crate::constants::web as c;
use crate::model::http_range::{self, Answer};
use crate::model::web_build::{self, Target};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

struct Request {
    method: String,
    path: String,
    range: Option<String>,
    close: bool,
}

/// Reads one request head; None at the connection's end or on a malformed head.
fn read_head(r: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    let mut total = 0;
    let mut req: Option<Request> = None;
    loop {
        line.clear();
        let n = r.read_line(&mut line).ok()?;
        total += n;
        if n == 0 || total > c::MAX_HEAD {
            return None;
        }
        let text = line.trim_end();
        let Some(rq) = req.as_mut() else {
            let (method, path) = http_range::request_line(text)?;
            let close = text.ends_with("HTTP/1.0");
            req = Some(Request {
                method,
                path,
                range: None,
                close,
            });
            continue;
        };
        if text.is_empty() {
            return req;
        }
        if let Some((k, v)) = text.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "range" => rq.range = Some(v.to_string()),
                "connection" => rq.close = v.eq_ignore_ascii_case("close"),
                _ => {}
            }
        }
    }
}

/// The file a path names: `/<slug>/<rest>` in a mounted build; Err(location) for `/<slug>` alone.
fn resolve(path: &str, mounts: &Mounts) -> Result<Option<PathBuf>, String> {
    let trimmed = path.trim_start_matches('/');
    let (slug, rest) = match trimmed.split_once('/') {
        Some((s, r)) => (s, Some(r)),
        None => (trimmed, None),
    };
    let mount: Option<Mount> = mounts.lock().ok().and_then(|m| m.get(slug).cloned());
    let Some(m) = mount else { return Ok(None) };
    let Some(rest) = rest else {
        return Err(format!("/{slug}/"));
    };
    Ok(match web_build::route(rest, &m.entry) {
        Some(Target::Build(p)) => Some(m.root.join(p)),
        Some(Target::Data(p)) => Some(m.data.join(p)),
        None => None,
    })
}

fn head(status: u16, headers: &[(&str, String)], length: u64) -> String {
    let mut h = format!("HTTP/1.1 {status} {}\r\n", http_range::reason(status));
    for (k, v) in c::ISOLATION_HEADERS {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("Accept-Ranges: bytes\r\nCache-Control: no-store\r\n");
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str(&format!("Content-Length: {length}\r\n\r\n"));
    h
}

/// Answers one request; Err when the connection broke.
fn answer(out: &mut TcpStream, req: &Request, mounts: &Mounts) -> std::io::Result<()> {
    let plain = |status: u16, extra: Vec<(&str, String)>| {
        let mut h = extra;
        h.push(("Content-Type", "text/plain".into()));
        head(status, &h, 0)
    };
    if req.method != "GET" && req.method != "HEAD" {
        let allow = vec![("Allow", "GET, HEAD".to_string())];
        return out.write_all(plain(405, allow).as_bytes());
    }
    let file = match resolve(&req.path, mounts) {
        Err(location) => return out.write_all(plain(301, vec![("Location", location)]).as_bytes()),
        Ok(f) => f.and_then(|p| File::open(&p).ok().map(|f| (p, f))),
    };
    let Some((path, mut f)) = file.filter(|(_, f)| f.metadata().is_ok_and(|m| m.is_file())) else {
        return out.write_all(plain(404, vec![]).as_bytes());
    };
    let size = f.metadata()?.len();
    let ctype = (
        "Content-Type",
        http_range::content_type(&path.to_string_lossy()).into(),
    );
    let (status, first, n, extra) = match http_range::answer(req.range.as_deref(), size) {
        Answer::Whole => (200, 0, size, vec![ctype]),
        Answer::Part(a, b) => {
            let cr = ("Content-Range", format!("bytes {a}-{b}/{size}"));
            (206, a, b - a + 1, vec![ctype, cr])
        }
        Answer::Unsatisfiable => {
            let cr = ("Content-Range", format!("bytes */{size}"));
            return out.write_all(plain(416, vec![cr]).as_bytes());
        }
    };
    out.write_all(head(status, &extra, n).as_bytes())?;
    if req.method == "HEAD" {
        return out.flush();
    }
    f.seek(SeekFrom::Start(first))?;
    copy(&mut f, out, n)
}

/// `n` bytes of the file to the connection, a chunk a write.
fn copy(f: &mut File, out: &mut TcpStream, mut n: u64) -> std::io::Result<()> {
    let mut buf = vec![0u8; c::CHUNK];
    while n > 0 {
        let want = usize::try_from(n).map_or(buf.len(), |n| n.min(buf.len()));
        let got = f.read(&mut buf[..want])?;
        if got == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        out.write_all(&buf[..got])?;
        n -= got as u64;
    }
    out.flush()
}

/// Serves a connection until it closes, errs or stays idle too long.
pub fn serve(stream: TcpStream, mounts: &Mounts) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(c::IDLE_SECONDS)));
    let _ = stream.set_nodelay(true);
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut r = BufReader::with_capacity(c::MAX_HEAD, stream);
    while let Some(req) = read_head(&mut r) {
        if answer(&mut out, &req, mounts).is_err() || req.close {
            return;
        }
    }
}

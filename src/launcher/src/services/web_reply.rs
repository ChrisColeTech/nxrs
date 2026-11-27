//! One connection of the web server: requests read until the browser closes it (keep-alive), each
//! answered with the cross-origin isolation headers, `Accept-Ranges`, an exact `206` and
//! `Content-Range` for a range, `416` past the end. GET and HEAD serve files; a POST (the page's
//! development log) is read and answered `204`. A whole-file GET that takes gzip gets the build's
//! `<file>.gz` with `Content-Encoding`; files carry an ETag and are revalidated (`304`).

use super::web_file;
use super::web_serve::{Mount, Mounts};
use crate::constants::web as c;
use crate::model::http_cache;
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
    /// the body's length (a POST)
    length: u64,
    accept: Option<String>,
    if_none_match: Option<String>,
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
                length: 0,
                accept: None,
                if_none_match: None,
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
                "content-length" => rq.length = v.parse().ok()?,
                "accept-encoding" => rq.accept = Some(v.to_string()),
                "if-none-match" => rq.if_none_match = Some(v.to_string()),
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
    h.push_str("Accept-Ranges: bytes\r\n");
    if !headers.iter().any(|(k, _)| *k == "Cache-Control") {
        h.push_str(&format!("Cache-Control: {}\r\n", c::CACHE_NONE));
    }
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
    // the page's development posts (the engine log, presented images: dev_server.py keeps them): taken, not kept
    if req.method == "POST" {
        return out.write_all(plain(204, vec![]).as_bytes());
    }
    if req.method != "GET" && req.method != "HEAD" {
        let allow = vec![("Allow", "GET, HEAD".to_string())];
        return out.write_all(plain(405, allow).as_bytes());
    }
    let file = match resolve(&req.path, mounts) {
        Err(location) => return out.write_all(plain(301, vec![("Location", location)]).as_bytes()),
        Ok(f) => f.and_then(|p| File::open(&p).ok().map(|f| (p, f))),
    };
    let Some((path, f)) = file.filter(|(_, f)| f.metadata().is_ok_and(|m| m.is_file())) else {
        return out.write_all(plain(404, vec![]).as_bytes());
    };
    let ctype = (
        "Content-Type",
        http_range::content_type(&path.to_string_lossy()).into(),
    );
    let (range, accept) = (req.range.as_deref(), req.accept.as_deref());
    let (mut f, gzip) = web_file::pick_encoding(range, accept, &path, f)?;
    let meta = f.metadata()?;
    let size = meta.len();
    let tag = http_cache::etag(size, web_file::mtime(&meta).unwrap_or(0), gzip);
    let mut extra = vec![
        ctype,
        ("Cache-Control", c::CACHE_FILES.into()),
        ("ETag", tag.clone()),
        ("Vary", "Accept-Encoding".into()),
    ];
    if http_cache::not_modified(req.if_none_match.as_deref(), req.range.as_deref(), &tag) {
        return out.write_all(head(304, &extra[1..], 0).as_bytes());
    }
    if gzip {
        extra.push(("Content-Encoding", "gzip".into()));
    }
    let (status, first, n) = match http_range::answer(req.range.as_deref(), size) {
        Answer::Whole => (200, 0, size),
        Answer::Part(a, b) => {
            extra.push(("Content-Range", format!("bytes {a}-{b}/{size}")));
            (206, a, b - a + 1)
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
    web_file::copy(&mut f, out, n)
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
        // a body is read past, so the next request starts where it should
        let body = std::io::copy(&mut (&mut r).take(req.length), &mut std::io::sink());
        if body.ok() != Some(req.length) || answer(&mut out, &req, mounts).is_err() || req.close {
            return;
        }
    }
}

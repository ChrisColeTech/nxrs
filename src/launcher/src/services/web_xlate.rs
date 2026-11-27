//! The web server's shader translation (WEB-W7b): a page whose library lacks a shader the game made
//! at run time POSTs it to `/<slug>/shader-xlate/<key>`; the mount's nxrs-build translates it with
//! the engine's own live translator (`nxrs-build shader-xlate <data>\shaders`, fixed arguments, the
//! body on stdin) into `<data>\shaders\live\<key>\`, the folder a native run's live translator
//! writes, and the SPIR-V and its json are the answer. Nothing else is run or written.

use super::piped_run;
use super::web_serve::Mounts;
use crate::constants::{web as c, words_web as w};
use crate::model::fill::fill;
use crate::model::http_range;
use crate::model::shader_xlate::{self as x, Route};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Whether a request is this route's (other POSTs are the page's development posts).
pub fn wants(method: &str, path: &str) -> bool {
    method == "POST" && x::route(path) != Route::Other
}

/// Answers one request of the route, reading its `length`-byte body; Ok(false) when the body was
/// refused unread (the connection is closed then).
pub fn serve(
    r: &mut impl Read,
    out: &mut impl Write,
    path: &str,
    length: u64,
    mounts: &Mounts,
) -> std::io::Result<bool> {
    if !x::length_ok(length) {
        let status = if length > c::MAX_XLATE_BODY { 413 } else { 400 };
        send(out, status, w::BAD_SHADER_REQUEST.as_bytes(), true)?;
        return Ok(false);
    }
    let mut body = Vec::new();
    r.take(length).read_to_end(&mut body)?;
    if body.len() as u64 != length {
        return Ok(false);
    }
    let (status, reply) = handle(path, &body, mounts);
    send(out, status, &reply, false)?;
    Ok(true)
}

fn handle(path: &str, body: &[u8], mounts: &Mounts) -> (u16, Vec<u8>) {
    let Route::Key { slug, key } = x::route(path) else {
        return (400, w::BAD_SHADER_REQUEST.into());
    };
    let Some(m) = mounts.lock().ok().and_then(|m| m.get(&slug).cloned()) else {
        return (404, Vec::new());
    };
    if !x::body_ok(body) {
        return (400, w::BAD_SHADER_REQUEST.into());
    }
    let shaders = m.data.join(c::SHADERS_FOLDER);
    let t0 = Instant::now();
    let _turn = Turn::take(shaders.join(c::LIVE_FOLDER).join(&key));
    let (status, reply, what) = if let Some(a) = translated(&shaders, &key) {
        (200, a, "already translated")
    } else if m.translator.is_empty() {
        (503, w::NO_TRANSLATOR.into(), "no translator")
    } else {
        match translate(&m.translator, &shaders, &key, body) {
            Ok(a) => (200, a, "translated"),
            Err(e) => (422, e.into_bytes(), "not translated"),
        }
    };
    let secs = format!("{:.2}", t0.elapsed().as_secs_f64());
    // the server's own record (`--serve-web` prints it; a window has no console, and loses nothing)
    let _ = writeln!(
        std::io::stdout(),
        "{}",
        fill(w::XLATE_LINE, &[&slug, &key, what, &secs])
    );
    (status, reply)
}

/// The answer from `<shaders>\live\<key>\spv\<key>.spv` and `.json`, when both are there.
fn translated(shaders: &Path, key: &str) -> Option<Vec<u8>> {
    let dir = shaders.join(c::LIVE_FOLDER).join(key).join(c::SPV_FOLDER);
    let spv = std::fs::read(dir.join(format!("{key}.spv"))).ok()?;
    let json = std::fs::read(dir.join(format!("{key}.json"))).ok()?;
    Some(x::answer(&spv, &json))
}

/// Runs `nxrs-build shader-xlate <shaders>` with the request on stdin, to its end or the time limit.
fn translate(program: &str, shaders: &Path, key: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    let args = [OsStr::new(c::XLATE_COMMAND), shaders.as_os_str()];
    let limit = Duration::from_secs(c::XLATE_SECONDS);
    let ran = piped_run::run(program, &args, body, limit)
        .map_err(|e| format!("{}{e}", w::XLATE_NOT_STARTED))?
        .ok_or_else(|| w::XLATE_LATE.to_string())?;
    if ran.code != Some(0) {
        return Err(ran.err.trim().to_string());
    }
    match x::printed_key(&ran.out) {
        Some(k) if k == key => translated(shaders, key).ok_or_else(|| ran.out.trim().to_string()),
        Some(k) => Err(fill(w::XLATE_OTHER_KEY, &[&k, key])),
        None => Err(ran.out.trim().to_string()),
    }
}

/// One answer: the isolation headers, never kept by the browser.
fn send(out: &mut impl Write, status: u16, body: &[u8], close: bool) -> std::io::Result<()> {
    let mut h = format!("HTTP/1.1 {status} {}\r\n", http_range::reason(status));
    for (k, v) in c::ISOLATION_HEADERS {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    let ctype = if status == 200 {
        "application/octet-stream"
    } else {
        "text/plain; charset=utf-8"
    };
    h.push_str(&format!(
        "Content-Type: {ctype}\r\nCache-Control: {}\r\n",
        c::CACHE_NONE
    ));
    if close {
        h.push_str("Connection: close\r\n");
    }
    h.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    out.write_all(h.as_bytes())?;
    out.write_all(body)?;
    out.flush()
}

/// One translation of a key at a time: a second request for it waits, then finds it translated.
struct Turn(PathBuf);

static BUSY: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
static FREED: Condvar = Condvar::new();

impl Turn {
    fn take(p: PathBuf) -> Turn {
        let mut g = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        while g.as_ref().is_some_and(|s| s.contains(&p)) {
            g = FREED.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        g.get_or_insert_with(HashSet::new).insert(p.clone());
        Turn(p)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut g = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_mut() {
            s.remove(&self.0);
        }
        FREED.notify_all();
    }
}

//! The parts of HTTP the web server decides without I/O: a `Range:` header against a file's size, a
//! file's content type, the request line.

/// What a GET of a file of `size` bytes answers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    /// 200: the whole file
    Whole,
    /// 206: bytes `first..=last`
    Part(u64, u64),
    /// 416: nothing of the file is in the range
    Unsatisfiable,
}

/// One `bytes=a-b`, `bytes=a-` or `bytes=-n` range (several ranges, or another unit, answer the whole
/// file, as HTTP allows).
pub fn answer(range: Option<&str>, size: u64) -> Answer {
    let Some(spec) = range.and_then(|r| r.trim().strip_prefix("bytes=")) else {
        return Answer::Whole;
    };
    if spec.contains(',') {
        return Answer::Whole;
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Answer::Whole;
    };
    let (a, b) = (a.trim(), b.trim());
    let (first, last) = match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(first), Ok(last)) if first <= last => (first, last.min(size.saturating_sub(1))),
        (Ok(_), Ok(_)) => return Answer::Whole,
        (Ok(first), Err(_)) if b.is_empty() => (first, size.saturating_sub(1)),
        (Err(_), Ok(n)) if a.is_empty() => {
            if n == 0 {
                return Answer::Unsatisfiable;
            }
            (size.saturating_sub(n), size.saturating_sub(1))
        }
        _ => return Answer::Whole,
    };
    if size == 0 || first >= size {
        return Answer::Unsatisfiable;
    }
    Answer::Part(first, last)
}

/// The content type by the file's extension.
pub fn content_type(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "wasm" => "application/wasm",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "json" => "application/json",
        "css" => "text/css; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "wgsl" | "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// The request line's method and path (the query dropped).
pub fn request_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next()?, parts.next()?);
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    path.starts_with('/')
        .then(|| (method.to_string(), path.to_string()))
}

/// The reason phrase of the statuses the server sends.
pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        _ => "Internal Server Error",
    }
}

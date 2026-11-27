//! What the web server decides about encodings and caching without I/O (docs/35 W6): a build's
//! precompressed copy (`<file>.gz`) for a whole-file GET that accepts gzip, and revalidation by
//! ETag, so a warm start is answered with 304s and the browser keeps its compiled module.

/// Whether an `Accept-Encoding` header takes gzip (a `q=0` refuses it).
pub fn accepts_gzip(header: Option<&str>) -> bool {
    header.is_some_and(|h| {
        h.split(',').any(|part| {
            let mut it = part.split(';').map(str::trim);
            let name = it.next().unwrap_or("");
            let q0 = it.any(|p| p.replace(' ', "").starts_with("q=0") && !p.contains("q=0."));
            (name.eq_ignore_ascii_case("gzip") || name == "*") && !q0
        })
    })
}

/// Whether the precompressed copy answers: a whole-file request that takes gzip, and a copy at
/// least as new as the file.
pub fn use_gzip(
    range: Option<&str>,
    accept: Option<&str>,
    file_mtime: u128,
    gz: Option<u128>,
) -> bool {
    range.is_none() && accepts_gzip(accept) && gz.is_some_and(|g| g >= file_mtime)
}

/// The entity tag of what is sent: its length and modification time, and whether it is encoded.
pub fn etag(len: u64, mtime_nanos: u128, gzip: bool) -> String {
    format!(
        "\"{len:x}-{mtime_nanos:x}{}\"",
        if gzip { "-gz" } else { "" }
    )
}

/// Whether `If-None-Match` names `tag` (a whole-file request only: a range is always sent).
pub fn not_modified(if_none_match: Option<&str>, range: Option<&str>, tag: &str) -> bool {
    range.is_none()
        && if_none_match.is_some_and(|h| h.split(',').any(|t| t.trim() == tag || t.trim() == "*"))
}

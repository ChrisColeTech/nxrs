//! Paths as the library holds them: absolute, cleaned, `/` separators; shown and passed to programs with
//! the platform's own. String work only (the current folder is read for a relative path).

/// `\` becomes `/` on Windows; elsewhere a backslash is an ordinary character.
pub fn from_native(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    }
}

/// `/` becomes `\` on Windows: what a user reads and a program is given.
pub fn native(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_string()
    }
}

/// "Not linked" for an empty path, else its native spelling.
pub fn display(path: &str) -> String {
    if path.is_empty() {
        crate::constants::words_library::NOT_LINKED.into()
    } else {
        native(path)
    }
}

fn is_absolute(path: &str) -> bool {
    let p = from_native(path);
    let b = p.as_bytes();
    if cfg!(windows) {
        (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()) || p.starts_with("//")
    } else {
        p.starts_with('/')
    }
}

/// The root part (`D:/`, `D:`, `//`, `/` or nothing) and the rest.
fn split_root(p: &str) -> (String, String) {
    let b = p.as_bytes();
    if cfg!(windows) && b.len() >= 2 && b[1] == b':' {
        let rest = &p[2..];
        if rest.starts_with('/') {
            return (
                format!("{}/", &p[..2]),
                rest.trim_start_matches('/').to_string(),
            );
        }
        return (p[..2].to_string(), rest.to_string());
    }
    if cfg!(windows) && p.starts_with("//") {
        return ("//".into(), p[2..].to_string());
    }
    if p.starts_with('/') {
        return ("/".into(), p.trim_start_matches('/').to_string());
    }
    (String::new(), p.to_string())
}

/// Collapses `//`, `.` and `..`; drops a trailing separator but keeps a root.
pub fn clean(path: &str) -> String {
    let p = from_native(path);
    if p.is_empty() {
        return p;
    }
    let (prefix, rest) = split_root(&p);
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            ".." if prefix.is_empty() => parts.push(".."),
            ".." => {}
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if prefix.is_empty() && joined.is_empty() {
        ".".into()
    } else {
        format!("{prefix}{joined}")
    }
}

/// Absolute against the current folder, cleaned; empty stays empty.
pub fn absolute(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let p = from_native(path);
    if is_absolute(&p) {
        return clean(&p);
    }
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    clean(&format!("{}/{p}", from_native(&cwd)))
}

/// `dir/name`, or `name` itself when it is absolute.
pub fn join(dir: &str, name: &str) -> String {
    if is_absolute(name) {
        return from_native(name);
    }
    let d = from_native(dir);
    if d.is_empty() {
        format!("./{name}")
    } else if d.ends_with('/') {
        format!("{d}{name}")
    } else {
        format!("{d}/{name}")
    }
}

/// The folder holding `path` (absolute).
pub fn parent(path: &str) -> String {
    let a = absolute(path);
    match a.rfind('/') {
        Some(0) => "/".into(),
        Some(i) if cfg!(windows) && i == 2 && a.as_bytes().get(1) == Some(&b':') => {
            a[..3].to_string()
        }
        Some(i) => a[..i].to_string(),
        None => a,
    }
}

pub fn file_name(path: &str) -> String {
    let p = from_native(path);
    p.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// The file name without its last suffix ("Game [v0].nsp" -> "Game [v0]").
pub fn base_name(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        Some(i) => name[..i].to_string(),
        None => name,
    }
}

/// The last suffix without its dot, lower case.
pub fn suffix(path: &str) -> String {
    let name = file_name(path);
    name.rfind('.')
        .map(|i| name[i + 1..].to_lowercase())
        .unwrap_or_default()
}

pub fn ends_with_ci(path: &str, tail: &str) -> bool {
    path.len() >= tail.len()
        && path.as_bytes()[path.len() - tail.len()..].eq_ignore_ascii_case(tail.as_bytes())
}

/// The same file: cleaned absolute paths, `/` and `\` alike, case-insensitive on Windows. Two empty
/// paths are the same; an empty one is never the same as another.
pub fn same_path(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return left == right;
    }
    let (l, r) = (absolute(left), absolute(right));
    if cfg!(windows) {
        l.to_lowercase() == r.to_lowercase()
    } else {
        l == r
    }
}

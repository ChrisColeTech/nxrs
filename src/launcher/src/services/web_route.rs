//! Which file a request names. An nxrs web build is served under `/<slug>/` on the server's own
//! address; an imported web game is served at the root of an address of its own,
//! `<slug>.localhost` (its pages load `/…` paths), chosen by the request's `Host`.

use super::web_serve::{Mount, Mounts};
use crate::constants::web as c;
use crate::model::web_build::{self, Target};
use std::path::PathBuf;

/// The host name a `Host` header names (its port dropped, lower case).
pub fn host_name(header: &str) -> String {
    let h = header.trim();
    let name = match h.rsplit_once(':') {
        Some((n, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => n,
        _ => h,
    };
    name.to_ascii_lowercase()
}

/// The mount key of a web game served at the root of `<slug>.localhost`.
pub fn site_key(slug: &str) -> String {
    format!("{slug}{}", c::SITE_SUFFIX)
}

fn file_of(m: &Mount, rest: &str) -> Option<PathBuf> {
    match web_build::route(rest, &m.entry)? {
        Target::Build(p) => Some(m.root.join(p)),
        Target::Data(p) => Some(m.data.join(p)),
    }
}

/// The file a request names: in the web game its host serves, else `/<slug>/<rest>` in a mounted
/// build; Err(location) for `/<slug>` alone.
pub fn resolve(path: &str, host: Option<&str>, mounts: &Mounts) -> Result<Option<PathBuf>, String> {
    let trimmed = path.trim_start_matches('/');
    let site = host
        .map(host_name)
        .filter(|h| h.ends_with(c::SITE_SUFFIX))
        .and_then(|h| mounts.lock().ok().and_then(|m| m.get(&h).cloned()));
    if let Some(m) = site {
        return Ok(file_of(&m, trimmed));
    }
    let (slug, rest) = match trimmed.split_once('/') {
        Some((s, r)) => (s, Some(r)),
        None => (trimmed, None),
    };
    let mount: Option<Mount> = mounts.lock().ok().and_then(|m| m.get(slug).cloned());
    let Some(m) = mount else { return Ok(None) };
    let Some(rest) = rest else {
        return Err(format!("/{slug}/"));
    };
    Ok(file_of(&m, rest))
}

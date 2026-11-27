//! A game's web build (docs/35 W5): where it goes, what its `build.json` says, the address it is
//! served under and which file a request names.

use super::fill::fill;
use super::game::Game;
use super::path_text::{join, parent};
use crate::constants::{web as c, words_web as w};

/// `<base>\nxrs-web\<name>` beside the game's `<base>\nxrs\<name>`; empty without a build folder.
pub fn web_dir(game: &Game) -> String {
    if game.build_dir.is_empty() || game.output_name.is_empty() {
        return String::new();
    }
    let base = parent(&parent(&game.build_dir));
    join(&join(&base, c::WEB_BUILDS_FOLDER), &game.output_name)
}

/// The page of an imported web game (`webBuild` names an `.html`/`.htm` file); None for an nxrs web
/// build (`webBuild` a folder, or none).
pub fn imported_page(game: &Game) -> Option<&str> {
    let p = game.web_build.as_str();
    let ext = super::path_text::suffix(p);
    (!p.is_empty() && c::PAGE_SUFFIXES.contains(&ext.as_str())).then_some(p)
}

/// A web game's address on a server at `port`: the root of its own `<slug>.localhost`.
pub fn site_url(port: u16, slug: &str) -> String {
    format!("http://{slug}{}:{port}/", c::SITE_SUFFIX)
}

/// What a server needs from `build.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct WebBuild {
    pub title: String,
    /// the data folder's name, beside `build.json`
    pub data: String,
    /// the page to open
    pub entry: String,
}

/// A plain folder name: no separators, no `.`/`..`, no drive.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

/// Reads `build.json`: refused when it is not one, is newer than this launcher or names no data folder.
pub fn parse(text: &str) -> Result<WebBuild, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| w::NOT_JSON.to_string())?;
    if v["kind"].as_str() != Some(c::BUILD_JSON_KIND) {
        return Err(w::NOT_WEB.into());
    }
    let format = v["format"].as_u64().unwrap_or(0);
    if format == 0 || format > c::BUILD_JSON_FORMAT {
        return Err(fill(w::NEWER, &[&format.to_string()]));
    }
    let data = v["data"].as_str().unwrap_or("").to_string();
    if !plain_name(&data) {
        return Err(w::NO_DATA.into());
    }
    let entry = v["entry"]
        .as_str()
        .filter(|e| plain_name(e))
        .unwrap_or(c::DEFAULT_ENTRY)
        .to_string();
    let title = v["title"].as_str().unwrap_or("").to_string();
    Ok(WebBuild { title, data, entry })
}

/// The build's first path segment on the server: lower-case letters and digits, `-` between words.
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() { "game".into() } else { s }
}

/// The browser a Windows `http` ProgId names, when it is Chromium-based (it then plays web builds).
pub fn chromium_browser(prog_id: &str) -> Option<&'static str> {
    c::CHROMIUM_PROG_IDS
        .iter()
        .find(|(id, _)| prog_id.eq_ignore_ascii_case(id))
        .map(|(_, name)| *name)
}

/// The page's address on a server at `port`.
pub fn url(port: u16, slug: &str) -> String {
    format!("http://{}:{port}/{slug}/", c::HOST)
}

/// A file a request names inside a served build.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// a path relative to the build folder (`/`-separated)
    Build(String),
    /// a path relative to the data folder
    Data(String),
}

/// `%XX` decoded; None when the text is not UTF-8 after decoding or an escape is broken.
pub fn percent_decode(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The file a path below the build's slug names (`""` is the entry page); None for anything that
/// could leave the folder (`..`, `.`, empty segments, `\`, `:`, NUL).
pub fn route(rest: &str, entry: &str) -> Option<Target> {
    let path = percent_decode(rest)?;
    if path.is_empty() {
        return Some(Target::Build(entry.to_string()));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if !segments.iter().all(|s| plain_name(s) && !s.contains('\0')) {
        return None;
    }
    match segments.split_first() {
        Some((first, rest)) if *first == c::DATA_SEGMENT && !rest.is_empty() => {
            Some(Target::Data(rest.join("/")))
        }
        _ => Some(Target::Build(path)),
    }
}

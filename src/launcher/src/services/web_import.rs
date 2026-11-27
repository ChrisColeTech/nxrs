//! Import build's web half: what a picked or dropped path is when it is a web build. An nxrs web
//! build is a folder with `build.json` and `game.wasm` (picked as the folder, `build.json` or its
//! page); any other web game is a page (`.html`), or a folder with an `index.html` at most
//! `PAGE_DEPTH` folders down (the GTA V port keeps it in `mirror\playgta5.com`).

use super::files::{is_dir, is_file, list, read_text};
use super::web_serve::{build_title_id, has_web_build};
use crate::constants::{engine as e, web as c, words_import as w};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::names::output_name_for;
use crate::model::path_text::{absolute, file_name, join, native, parent, suffix};

/// A web build found at a path.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// an nxrs web build's folder
    Nxrs(String),
    /// a web game's page
    Page(String),
}

fn is_page(path: &str) -> bool {
    c::PAGE_SUFFIXES.contains(&suffix(path).as_str())
}

/// What a folder holds at its top: an nxrs web build, or a page served at its address.
fn at_top(dir: &str) -> Option<Found> {
    if has_web_build(dir) {
        return Some(Found::Nxrs(dir.to_string()));
    }
    let index = join(dir, c::DEFAULT_ENTRY);
    is_file(&index).then_some(Found::Page(index))
}

/// The web build at `path`; Err with why it is none (a path that is not one at all: `NOT_A_BUILD`).
pub fn find(path: &str) -> Result<Found, String> {
    let path = absolute(path);
    if is_file(&path) {
        let dir = parent(&path);
        if file_name(&path).eq_ignore_ascii_case(c::BUILD_JSON) || is_page(&path) {
            if has_web_build(&dir) {
                return Ok(Found::Nxrs(dir));
            }
            if is_page(&path) {
                return Ok(Found::Page(path));
            }
        }
        return Err(w::NOT_A_BUILD.into());
    }
    if !is_dir(&path) {
        return Err(w::NOT_A_BUILD.into());
    }
    // level by level: the shallowest web build wins; two at one level are a question for the player
    let mut level = vec![path.clone()];
    for _ in 0..=c::PAGE_DEPTH {
        let found: Vec<Found> = level.iter().filter_map(|d| at_top(d)).collect();
        if let [one] = found.as_slice() {
            return Ok(one.clone());
        }
        if !found.is_empty() {
            let names: Vec<String> = found.iter().map(|f| native(where_of(f))).collect();
            return Err(format!("{}{}", w::SEVERAL_PAGES, names.join(", ")));
        }
        level = level
            .iter()
            .flat_map(|d| {
                list(d)
                    .into_iter()
                    .filter(|(_, folder)| *folder)
                    .map(|(name, _)| join(d, &name))
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    Err(fill(w::NO_PAGE, &[&c::PAGE_DEPTH.to_string()]))
}

fn where_of(f: &Found) -> &str {
    match f {
        Found::Nxrs(d) | Found::Page(d) => d,
    }
}

/// A new library entry for a found web build: titled by `build.json`, else by the picked folder (a
/// page picked itself: its folder).
pub fn game_for(found: &Found, picked: &str) -> Game {
    let picked = absolute(picked);
    let named = if is_dir(&picked) {
        file_name(&picked)
    } else {
        file_name(&parent(&picked))
    };
    let (web_build, title) = match found {
        Found::Nxrs(dir) => (dir.clone(), nxrs_title(dir).unwrap_or(named)),
        Found::Page(page) => (page.clone(), named),
    };
    Game {
        id: uuid::Uuid::new_v4().to_string(),
        output_name: output_name_for(&title),
        title,
        engine: e::ENGINE_NXRS.into(),
        web_build,
        ..Game::default()
    }
}

fn nxrs_title(dir: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&read_text(&join(dir, c::BUILD_JSON))?).ok()?;
    let t = v.get("title")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The title id an nxrs web build was made from (lower case), to join it to that game's entry.
pub fn title_id(found: &Found) -> Option<String> {
    match found {
        Found::Nxrs(dir) => build_title_id(dir),
        Found::Page(_) => None,
    }
}

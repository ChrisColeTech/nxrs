//! The launcher's own copies of covers and backgrounds: `<data folder>\artwork\<entry id>-<kind>.<ext>`.
//! An entry's artwork never points at where the image was picked from, so moving or deleting the
//! original never breaks it. A copy is replaced when the art is, and removed with its entry.

use super::files::{is_file, list, write_atomic};
use crate::constants::{engine as e, words_import as w};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::path_text::{file_name, from_native, join, native, parent, suffix};

/// The folder beside the library file (`<data folder>\artwork`, or beside `--library`).
pub fn folder(library: &str) -> String {
    join(&parent(library), e::ARTWORK_FOLDER)
}

/// The entry's own copy's name without its extension: `<id>-cover`, `<id>-background`.
fn stem(id: &str, kind: &str) -> String {
    format!("{id}-{kind}")
}

/// `path` is the entry's own copy of that kind (any extension) in `dir`.
pub fn is_own(dir: &str, id: &str, kind: &str, path: &str) -> bool {
    let name = file_name(path);
    let own = stem(id, kind);
    crate::model::path_text::same_path(&parent(path), dir)
        && (name == own || name.strip_prefix(&own).is_some_and(|r| r.starts_with('.')))
}

/// The image at `path` copied in as the entry's `kind`; the copy's path. An empty path, or the
/// entry's own copy already, stays as it is; an image that cannot be read is an error (the caller
/// keeps the path it had). Other copies of that kind (another extension) are removed.
pub fn adopt(dir: &str, id: &str, kind: &str, path: &str) -> Result<String, String> {
    if path.is_empty() || is_own(dir, id, kind, path) {
        return Ok(path.to_string());
    }
    if !is_file(path) {
        return Err(fill(w::ART_MISSING, &[&native(path)]));
    }
    let bytes =
        std::fs::read(native(path)).map_err(|err| fill(w::ART_UNREADABLE, &[&err.to_string()]))?;
    let ext = suffix(path);
    let name = if ext.is_empty() {
        stem(id, kind)
    } else {
        format!("{}.{ext}", stem(id, kind))
    };
    let copy = join(dir, &name);
    write_atomic(&native(&copy), &bytes).map_err(|err| fill(w::ART_UNREADABLE, &[&err]))?;
    remove_matching(dir, id, kind, Some(&name));
    Ok(from_native(&copy))
}

/// Removes the entry's copies of `kind`, except `keep`.
fn remove_matching(dir: &str, id: &str, kind: &str, keep: Option<&str>) {
    for (name, folder) in list(dir) {
        let path = join(dir, &name);
        if !folder && Some(name.as_str()) != keep && is_own(dir, id, kind, &path) {
            let _ = std::fs::remove_file(native(&path));
        }
    }
}

/// Removes every copy an entry has (its entry left the library).
pub fn remove_copies(dir: &str, id: &str) {
    for kind in e::ART_KINDS {
        remove_matching(dir, id, kind, None);
    }
}

/// What owning a game's artwork did: whether it changed, and a line per image left as it was.
#[derive(Debug, Default, PartialEq)]
pub struct Owned {
    pub changed: bool,
    pub left: Vec<String>,
}

/// Copies the game's cover and background in and points the entry at the copies. A cleared kind
/// loses its copy; an image that cannot be copied is left as it is (and reported), never blanked.
pub fn own(dir: &str, g: &mut Game) -> Owned {
    let mut out = Owned::default();
    for kind in e::ART_KINDS {
        let current = if kind == e::ART_KINDS[0] {
            g.cover.clone()
        } else {
            g.background.clone()
        };
        if current.is_empty() {
            remove_matching(dir, &g.id, kind, None);
            continue;
        }
        match adopt(dir, &g.id, kind, &current) {
            Ok(copy) if copy != current => {
                if kind == e::ART_KINDS[0] {
                    g.cover = copy;
                } else {
                    g.background = copy;
                }
                out.changed = true;
            }
            Ok(_) => {}
            Err(err) => out.left.push(fill(w::ART_LEFT, &[&g.title, kind, &err])),
        }
    }
    out
}

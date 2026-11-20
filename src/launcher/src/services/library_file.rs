//! `library.json` on disk: read as the Qt launcher reads it (a missing file is an empty library; an
//! unreadable or invalid one is an error and is never overwritten), written atomically in its format.
//! The first write over a file the Qt launcher wrote last copies it to `library.qt-backup.json`.

use super::files::{exists, read_bytes, write_atomic};
use super::{build_facts, ini_file, markers};
use crate::constants::{engine as e, words_library as w};
use crate::model::json_fields::int_of;
use crate::model::library::Library;
use crate::model::library_json::{self, OWN_KEY};
use crate::model::path_text::{join, parent};
use crate::model::{dates, game_json, qtjson};
use serde_json::Value;

/// Whether the file may be written, and whether its first write has backed it up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Store {
    pub writable: bool,
    pub backed_up: bool,
}
impl Default for Store {
    fn default() -> Self {
        Store {
            writable: true,
            backed_up: false,
        }
    }
}

/// Reads `lib.path` into `lib`. An error leaves the store unwritable.
pub fn load(lib: &mut Library, store: &mut Store) -> Result<(), String> {
    let bytes = match std::fs::read(&lib.path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            store.writable = false;
            return Err(err.to_string());
        }
    };
    let doc: Option<Value> = serde_json::from_slice(&bytes).ok();
    let Some(o) = doc
        .as_ref()
        .and_then(Value::as_object)
        .filter(|o| int_of(o, "schema", 0) == 1)
    else {
        store.writable = false;
        return Err(format!("{}{}", w::INVALID_FILE, lib.path));
    };
    library_json::settings_from(o, lib);
    lib.games.clear();
    for value in o
        .get("games")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut game = game_json::from_json(value);
        markers::adopt_built_update(&mut game);
        // a library from before the screen mode was kept here: a handheld choice in a built game's
        // INI is that game's own
        let had_mode = value
            .as_object()
            .is_some_and(|g| g.contains_key("screenMode"));
        let handheld = exists(&game.ini_path()) && !ini_file::read_docked(&game.ini_path());
        if !had_mode && build_facts::has_build(&game) && handheld {
            game.screen_mode = "handheld".into();
        }
        if !game.id.is_empty() && !game.title.is_empty() && lib.find(&game.id).is_none() {
            lib.games.push(game);
        }
    }
    Ok(())
}

/// Writes the library: sorted keys, 4-space indent, atomically.
pub fn save(lib: &Library, store: &mut Store) -> Result<(), String> {
    if !store.writable {
        return Err(w::NOT_WRITABLE.into());
    }
    let dir = parent(&lib.path);
    std::fs::create_dir_all(&dir).map_err(|_| w::NO_DIRECTORY.to_string())?;
    if !store.backed_up {
        back_up_qt_file(&lib.path, &dir)?;
        store.backed_up = true;
    }
    write_atomic(
        &lib.path,
        qtjson::to_string(&library_json::to_json(lib)).as_bytes(),
    )
}

/// The Qt launcher wrote the file last (it has no `consolePanel`): copy it aside once.
fn back_up_qt_file(path: &str, dir: &str) -> Result<(), String> {
    let Ok(bytes) = read_bytes(path) else {
        return Ok(());
    };
    let by_qt = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.as_object().map(|o| !o.contains_key(OWN_KEY)))
        .unwrap_or(false);
    if !by_qt {
        return Ok(());
    }
    let mut backup = join(dir, &format!("{}.json", e::QT_BACKUP));
    if exists(&backup) {
        backup = join(dir, &format!("{}-{}.json", e::QT_BACKUP, dates::stamp()));
    }
    std::fs::write(&backup, &bytes).map_err(|err| format!("Cannot back up the library file: {err}"))
}

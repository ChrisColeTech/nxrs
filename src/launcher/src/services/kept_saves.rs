//! Remove from library with "Also delete the build": the player's files leave the build first and wait in
//! `<data folder>\saves\<title id>\`; building the game again puts them back.
//!
//! Kept: each data folder's (`<name>_data`, `<name>_p<N>_data`) `save\` (the game's saves), `sdmc\` (its SD
//! card), `cheats\` (cheat files and their toggles), and the build folder's `mods\`. Everything else in a
//! build is the engine's output and is built again.

use super::files::{is_dir, read_at};
use crate::constants::engine as e;
use crate::model::game::Game;
use crate::model::path_text::{absolute, file_name, join, native, parent, same_path};
use std::path::Path;

fn io(path: &str) -> impl Fn(std::io::Error) -> String + '_ {
    move |err| format!("{}: {err}", native(path))
}

/// The game's program id (16 lowercase hex digits) from the build's `exefs/main.npdm` (ACI0).
pub fn title_id(data_dir: &str) -> Option<String> {
    let npdm = join(data_dir, e::NPDM);
    let head = read_at(&npdm, 0, 0x80).ok()?;
    if head.get(..4) != Some(b"META") {
        return None;
    }
    let aci = u64::from(u32::from_le_bytes(head.get(0x70..0x74)?.try_into().ok()?));
    let aci0 = read_at(&npdm, aci, 0x18).ok()?;
    if aci0.get(..4) != Some(b"ACI0") {
        return None;
    }
    let id = u64::from_le_bytes(aci0.get(0x10..0x18)?.try_into().ok()?);
    Some(format!("{id:016x}"))
}

/// The bytes of the files under a folder.
pub fn folder_size(path: &str) -> u64 {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(m) if m.is_dir() => folder_size(&entry.path().to_string_lossy()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// (place in the build, place in the kept folder) of every player folder the build has.
fn player_folders(game: &Game) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let prefix = format!("{}_", game.output_name);
    for entry in std::fs::read_dir(&game.build_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(middle) = name
            .strip_prefix(&prefix)
            .and_then(|n| n.strip_suffix("data"))
        else {
            continue;
        };
        // `<name>_data` -> data, `<name>_p1_data` -> p1
        let program = middle.trim_end_matches('_');
        let program = if program.is_empty() { "data" } else { program };
        for kept in e::PLAYER_FOLDERS {
            let from = join(&join(&game.build_dir, &name), kept);
            if is_dir(&from) {
                out.push((from, format!("{program}/{kept}")));
            }
        }
    }
    let mods = join(&game.build_dir, e::MODS_FOLDER);
    if is_dir(&mods) {
        out.push((mods, e::MODS_FOLDER.to_string()));
    }
    out
}

/// Move a folder; across drives it is copied, then removed.
fn move_folder(from: &str, to: &str) -> Result<(), String> {
    std::fs::create_dir_all(parent(to)).map_err(io(to))?;
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    super::files::copy_tree(from, to, &mut 0)?;
    std::fs::remove_dir_all(from).map_err(io(from))
}

/// Moves the player's folders out of the build into `<saves root>\<title id>` (an older kept folder of
/// the same game is moved aside, never merged). The kept folder, or None when there was nothing to keep.
pub fn keep(game: &Game, saves_root: &str) -> Result<Option<String>, String> {
    let found = player_folders(game);
    if found.is_empty() {
        return Ok(None);
    }
    let key = title_id(&game.data_dir()).unwrap_or_else(|| game.output_name.clone());
    let kept = join(saves_root, &key);
    if Path::new(&kept).exists() {
        let aside = format!("{kept}.{}", crate::model::dates::stamp());
        std::fs::rename(&kept, &aside).map_err(io(&kept))?;
    }
    for (from, to) in found {
        move_folder(&from, &join(&kept, &to))?;
    }
    Ok(Some(kept))
}

/// After a build: the kept player's folders of this game go back into it, where the build has none of
/// its own (a folder the new build already has is left, and so is the kept one). The kept folder, when
/// something was put back.
pub fn restore(game: &Game, saves_root: &str) -> Result<Option<String>, String> {
    let key = title_id(&game.data_dir()).unwrap_or_else(|| game.output_name.clone());
    let kept = join(saves_root, &key);
    if !is_dir(&kept) {
        return Ok(None);
    }
    let mut moved = false;
    for entry in std::fs::read_dir(&kept).map_err(io(&kept))?.flatten() {
        let program = entry.file_name().to_string_lossy().into_owned();
        let from = join(&kept, &program);
        if program == e::MODS_FOLDER {
            moved |= put_back(&from, &join(&game.build_dir, e::MODS_FOLDER))?;
            continue;
        }
        let data = if program == "data" {
            game.data_dir()
        } else {
            join(
                &game.build_dir,
                &format!("{}_{program}_data", game.output_name),
            )
        };
        for name in e::PLAYER_FOLDERS {
            moved |= put_back(&join(&from, name), &join(&data, name))?;
        }
        let _ = std::fs::remove_dir(&from);
    }
    let _ = std::fs::remove_dir(&kept);
    Ok(moved.then_some(kept))
}

/// `from` into `to` when `from` is there and `to` is not (or is an empty folder).
fn put_back(from: &str, to: &str) -> Result<bool, String> {
    if !is_dir(from) {
        return Ok(false);
    }
    let empty = std::fs::read_dir(to).is_ok_and(|mut d| d.next().is_none());
    if empty {
        std::fs::remove_dir(to).map_err(io(to))?;
    } else if Path::new(to).exists() {
        return Ok(false);
    }
    move_folder(from, to).map(|()| true)
}

/// Deletes the game's build folder, only it: refused when it holds the source, has no build of this
/// game in it, or is a drive's root. An `nxrs` folder left empty above it goes too.
pub fn delete_build(game: &Game) -> Result<(), String> {
    let dir = absolute(&game.build_dir);
    let source = absolute(&game.source);
    let holds_source = !game.source.is_empty()
        && (same_path(&dir, &source) || source.starts_with(&format!("{dir}/")));
    let ours = is_dir(&game.data_dir())
        || Path::new(&game.executable()).is_file()
        || is_dir(&join(&dir, &format!("{}_build", game.output_name)));
    if holds_source || !ours || same_path(&parent(&dir), &dir) {
        return Err(format!("{}{}", e::NOT_A_BUILD, native(&dir)));
    }
    std::fs::remove_dir_all(&dir).map_err(io(&dir))?;
    let holder = parent(&dir);
    if file_name(&holder) == e::BUILDS_FOLDER {
        let _ = std::fs::remove_dir(&holder);
    }
    Ok(())
}

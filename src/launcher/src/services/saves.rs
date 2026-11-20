//! Importing a game's Turbo saves into its nxrs build: `<build folder>\<name>_data\save\` of the Turbo
//! build into the same in `<build folder> [nxrs]`, after a backup of the nxrs build's saves. The Turbo
//! build is only read.

use super::files::{copy_tree, exists, has_files, is_dir, remove_tree};
use crate::constants::words_play as w;
use crate::model::dates;
use crate::model::fill::{fill, plural};
use crate::model::game::Game;
use crate::model::names::build_folder_for;
use crate::model::path_text::{join, native, parent};
use crate::model::save_plan::{self, STALE, SaveFolderPair, save_folder};

#[derive(Debug)]
pub struct SaveCopy {
    /// the backup of the nxrs build's saves, empty when it had none
    pub backup: String,
    pub report: String,
}

fn plan(game: &Game) -> Vec<SaveFolderPair> {
    let from = save_folder(&build_folder_for(&game.build_dir, false), &game.output_name);
    let to = save_folder(&game.build_dir, &game.output_name);
    save_plan::plan(&from, &to, |name| is_dir(&join(&from, name)))
}

/// The game's Turbo build holds saves its nxrs build can take.
pub fn turbo_build_has_saves(game: &Game) -> bool {
    game.nxrs()
        && !game.build_dir.is_empty()
        && !game.output_name.is_empty()
        && plan(game).iter().any(|p| has_files(&p.from))
}

pub fn copy_from_turbo(game: &Game) -> Result<SaveCopy, String> {
    if !game.nxrs() {
        return Err(w::NOT_NXRS.into());
    }
    if !is_dir(&game.data_dir()) {
        return Err(w::NO_DATA.into());
    }
    let pairs = plan(game);
    if !pairs.iter().any(|p| has_files(&p.from)) {
        let from = save_folder(&build_folder_for(&game.build_dir, false), &game.output_name);
        return Err(fill(w::NOTHING, &[&native(&from)]));
    }
    let to_save = save_folder(&game.build_dir, &game.output_name);
    let backup = back_up(&to_save)?;
    let mut lines = Vec::new();
    for pair in pairs.iter().filter(|p| has_files(&p.from)) {
        // nxrs's transaction folders beside the destination would be taken for an unfinished commit
        for stale in STALE.iter().map(|s| format!("{}{s}", pair.to)) {
            if is_dir(&stale) && !remove_tree(&stale) {
                return Err(format!("{}{}", w::CANNOT_REMOVE, native(&stale)));
            }
        }
        if is_dir(&pair.to) && !remove_tree(&pair.to) {
            return Err(format!("{}{}", w::CANNOT_REPLACE, native(&pair.to)));
        }
        let mut n = 0;
        copy_tree(&pair.from, &pair.to, &mut n)?;
        lines.push(format!(
            "{} -> {} ({n} file{})",
            native(&pair.from),
            native(&pair.to),
            plural(n)
        ));
    }
    Ok(SaveCopy {
        backup,
        report: lines.join("\n"),
    })
}

/// The whole save folder, as it is, to `save-backup-<time>` beside it; empty when it had no files.
fn back_up(to_save: &str) -> Result<String, String> {
    if !is_dir(to_save) || !has_files(to_save) {
        return Ok(String::new());
    }
    let target = join(&parent(to_save), &format!("save-backup-{}", dates::stamp()));
    if exists(&target) {
        return Err(format!("{}{}", w::BACKUP_EXISTS, native(&target)));
    }
    let mut n = 0;
    copy_tree(to_save, &target, &mut n)?;
    Ok(target)
}

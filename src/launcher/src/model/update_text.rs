//! What the update lists and status lines say (`updateTokenLabel`, `updateDescription`,
//! `updateOutcome`).

use super::fill::fill;
use super::game::Game;
use super::game_update::GameUpdate;
use super::path_text::{file_name, same_path};
use crate::constants::words_updates as w;

/// "update 1.1.1 (v262144)", "the game without updates", "the game as its package is".
pub fn token_label(game: &Game, token: &str) -> String {
    if token == "none" {
        return w::WITHOUT_LABEL.into();
    }
    if let Some(u) = game.updates.iter().find(|u| same_path(&u.path, token)) {
        return format!("update {}", u.label());
    }
    if token.is_empty() {
        w::AS_IS.into()
    } else {
        format!("update {}", file_name(token))
    }
}

/// "Update [v262144].nsp — 1.1.1 (v262144) — applied". `exists`: the package is there; `built`: the
/// game's executable is.
pub fn description(game: &Game, update: &GameUpdate, exists: bool, built: bool) -> String {
    let mut text = format!("{} — {}", file_name(&update.path), update.label());
    let chosen = game
        .active_update()
        .is_some_and(|a| same_path(&a.path, &update.path));
    let applied = !game.built_update.is_empty()
        && game.built_update != "none"
        && same_path(&game.built_update, &update.path)
        && !game.build_incomplete
        && built;
    let tail = match (exists, chosen, applied) {
        (false, ..) => crate::constants::words_dlc::FILE_NOT_FOUND,
        (true, true, true) => w::APPLIED,
        (true, true, false) => w::SELECTED,
        (true, false, true) => w::IN_BUILD,
        _ => "",
    };
    text.push_str(tail);
    text
}

/// What the game is built as and what the next build will be, when they differ.
pub fn pending(game: &Game, update_pending: bool) -> String {
    if !update_pending {
        return String::new();
    }
    fill(
        w::PENDING,
        &[
            &token_label(game, &game.built_update),
            &token_label(game, &game.update_token()),
        ],
    )
}

/// Whether a rebuild is needed for the chosen version.
pub fn outcome(game: &Game, update_pending: bool, has_build: bool) -> String {
    let next = token_label(game, &game.update_token());
    let template = if update_pending {
        w::OUTCOME_PENDING
    } else if has_build {
        w::OUTCOME_BUILT
    } else {
        w::OUTCOME_NEXT
    };
    fill(template, &[&next])
}

/// The "Version to build" choices: (label, value), "" newest, each package, "none".
pub fn choices(game: &Game) -> Vec<(String, String)> {
    let newest = game
        .updates
        .iter()
        .fold(None::<&GameUpdate>, |n, u| match n {
            Some(n) if n.version >= u.version => Some(n),
            _ => Some(u),
        });
    let head = newest.map_or_else(
        || w::NEWEST.to_string(),
        |n| format!("{} — {}", w::NEWEST, n.label()),
    );
    let mut v = vec![(head, String::new())];
    v.extend(game.updates.iter().map(|u| {
        (
            format!("{} — {}", u.label(), file_name(&u.path)),
            u.path.clone(),
        )
    }));
    v.push((w::WITHOUT.into(), "none".into()));
    v
}

/// The choice shown now: its index in `choices`.
pub fn choice_index(game: &Game) -> usize {
    if game.update_choice == "none" {
        return game.updates.len() + 1;
    }
    let chosen = &game.update_choice;
    game.updates
        .iter()
        .position(|u| !chosen.is_empty() && same_path(&u.path, chosen))
        .map_or(0, |i| i + 1)
}

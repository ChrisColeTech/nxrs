//! What the disk says about a game's build (`Game::hasBuild`, `updatePending`, `version`).

use super::files::{is_dir, is_file, read_at};
use crate::constants::engine as e;
use crate::model::facts::Facts;
use crate::model::game::{Game, in_place_package, same_token};
use crate::model::path_text::{join, same_path};

/// Its executable, code module and data folder are there, the build finished and was made from its
/// source (or it has none).
fn built_files(g: &Game) -> bool {
    !g.build_incomplete
        && !g.build_dir.is_empty()
        && !g.output_name.is_empty()
        && is_file(&g.executable())
        && is_file(&g.code_module())
        && is_dir(&g.data_dir())
        && (g.source.is_empty() || same_path(&g.built_source, &g.source))
}

pub fn has_build(g: &Game) -> bool {
    built_files(g) && same_token(&g.built_update, &g.update_token())
}

/// Built, but with another update than the one chosen now: build again.
pub fn update_pending(g: &Game) -> bool {
    built_files(g) && !same_token(&g.built_update, &g.update_token())
}

pub fn source_available(g: &Game) -> bool {
    super::files::exists(&g.source)
}

/// Play can start it: an nxrs build that is there.
pub fn playable(g: &Game) -> bool {
    g.nxrs() && has_build(g)
}

pub fn facts(g: &Game) -> Facts {
    Facts {
        has_build: has_build(g),
        update_pending: update_pending(g),
        source_available: source_available(g),
        in_place: in_place_package(&g.source),
        build_incomplete: g.build_incomplete,
    }
}

/// The built game's display version ("1.1.1"), from the control data the build staged.
pub fn version(g: &Game) -> String {
    let Ok(raw) = read_at(&join(&g.data_dir(), e::NACP), e::NACP_VERSION_AT, 16) else {
        return String::new();
    };
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    let text = String::from_utf8_lossy(&raw[..end]).into_owned();
    if text.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        text
    } else {
        String::new()
    }
}

/// The game's own `<name>.log` is there.
pub fn has_log(g: &Game) -> bool {
    super::files::exists(&join(&g.build_dir, &format!("{}.log", g.output_name)))
}

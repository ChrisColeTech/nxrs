//! The names the launcher gives: a game's title from its file or folder, its output (executable) name.

use crate::constants::words_library as w;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

/// `[...]` tags and the space before them.
static TAGS: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"\s*\[[^\]]*\]").ok());

/// ASCII only, as the engine names its output: an accented letter loses its accent ("Pokémon" ->
/// "Pokemon"); letters, digits, `-`, `_` and spaces stay; nothing left is "Game".
pub fn output_name_for(title: &str) -> String {
    let kept: String = title
        .nfkd()
        .filter(|c| c.is_ascii_alphanumeric() || "-_ ".contains(*c))
        .collect();
    let trimmed = kept.trim();
    if trimmed.is_empty() {
        w::DEFAULT_NAME.into()
    } else {
        trimmed.to_string()
    }
}

/// `<folder> [nxrs]`, the folder a build goes into (once: a folder that has the suffix keeps it).
pub fn build_folder_for(build_dir: &str) -> String {
    let suffix = crate::constants::engine::NXRS_FOLDER_SUFFIX;
    let mut base = super::path_text::from_native(build_dir);
    while base.ends_with('/') {
        base.pop();
    }
    if let Some(stripped) = base.strip_suffix(suffix) {
        base = stripped.to_string();
    }
    base + suffix
}

/// A title from a file or folder name (no suffix for a file): `[...]` tags removed.
pub fn title_from_name(name: &str) -> String {
    let cleaned = match TAGS.as_ref() {
        Some(re) => re.replace_all(name, "").into_owned(),
        None => name.to_string(),
    };
    let t = cleaned.trim();
    if t.is_empty() {
        w::UNTITLED.into()
    } else {
        t.to_string()
    }
}

//! What the launcher reads in nxrs-build's output: progress, the package-holds-DLC line, the update
//! check's answers.

use super::game_update::GameUpdate;
use super::path_text::from_native;
use crate::constants::engine as e;
use std::sync::LazyLock;

static COUNTS: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"(?:C generated:|compiled)\s+(\d+)/(\d+)").ok());

/// A progress line: (label, done, total). `@progress\t<stage>\t<done>\t<total>`, or the text lines
/// the engine also prints.
pub fn progress(line: &str) -> Option<(String, i64, i64)> {
    if let Some(rest) = line.strip_prefix("@progress\t") {
        let parts: Vec<&str> = rest.split('\t').collect();
        let stage = parts.first().copied().unwrap_or("");
        let label = e::STAGES
            .iter()
            .find(|(s, _)| *s == stage)
            .map_or(stage, |(_, l)| *l);
        let n = |i: usize| {
            parts
                .get(i)
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0)
        };
        return Some((label.to_string(), n(1), n(2)));
    }
    if line.starts_with('@') {
        return None;
    }
    if let Some(m) = COUNTS.as_ref().and_then(|re| re.captures(line)) {
        let label = if line.contains("C generated") {
            "Generating native C"
        } else {
            "Compiling native code"
        };
        let n = |i: usize| m.get(i).and_then(|x| x.as_str().parse().ok()).unwrap_or(0);
        return Some((label.into(), n(1), n(2)));
    }
    let simple = [
        ("Analyzing", "Analyzing ARM64 code"),
        ("Linking", "Linking native module"),
        ("RomFS:", "Preparing game data"),
    ];
    simple
        .iter()
        .find(|(p, _)| line.starts_with(p))
        .map(|(_, l)| (l.to_string(), 0, 0))
}

/// The engine said the package just built also holds add-on content.
pub fn reports_package_dlc(line: &str) -> bool {
    line.starts_with(e::PACKAGE_DLC_LINE)
}

#[derive(Debug, PartialEq)]
pub enum UpdateLine {
    Found(GameUpdate),
    Error(String),
    Other,
}

/// `UPDATE\t<path>\t<title id>\t<version>\t<display>` or `UPDATE error: <reason>`.
pub fn update_line(line: &str) -> UpdateLine {
    if let Some(reason) = line.strip_prefix(e::UPDATE_ERROR) {
        return UpdateLine::Error(reason.trim().to_string());
    }
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 4 || parts.first() != Some(&"UPDATE") {
        return UpdateLine::Other;
    }
    let field = |i: usize| {
        parts
            .get(i)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let update = GameUpdate {
        path: from_native(&field(1)),
        title_id: field(2),
        version: field(3).parse().unwrap_or(0),
        display: field(4),
    };
    if update.path.is_empty() {
        UpdateLine::Other
    } else {
        UpdateLine::Found(update)
    }
}

/// A `DLC error: <reason>` line's reason.
pub fn dlc_error(line: &str) -> Option<&str> {
    line.strip_prefix(e::DLC_ERROR)
}

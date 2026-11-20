//! Add-on content installed in a built game's data folder: the lines of `dlc.manifest`
//! (`<id16> <version> [data|stub] <package>`) and the lists' descriptions.

use super::path_text::{file_name, same_path};
use crate::constants::words_dlc as w;
use std::sync::LazyLock;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DlcTitle {
    /// 16 hex digits, lower case
    pub title_id: String,
    pub version: i64,
    /// installed without its data (diagnostics only)
    pub stub: bool,
    /// the package it came from
    pub source: String,
}

static LINE: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"^([0-9A-Fa-f]{16})[ \t]+(\d+)(?:[ \t]+(.*))?$").ok());
static WORD: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"^(data|stub)(?:[ \t]+(.*))?$").ok());

/// The titles a manifest's text lists; comments and anything else are skipped.
pub fn parse_manifest(text: &str) -> Vec<DlcTitle> {
    let (Some(line), Some(word)) = (LINE.as_ref(), WORD.as_ref()) else {
        return Vec::new();
    };
    let mut titles = Vec::new();
    for raw in text.split('\n') {
        let Some(m) = line.captures(raw.trim()) else {
            continue;
        };
        let group = |i: usize| {
            m.get(i)
                .map(|x| x.as_str().trim().to_string())
                .unwrap_or_default()
        };
        let mut rest = group(3);
        let mut stub = false;
        if let Some(s) = word.captures(&rest.clone()) {
            stub = s.get(1).is_some_and(|x| x.as_str() == "stub");
            rest = s
                .get(2)
                .map(|x| x.as_str().trim().to_string())
                .unwrap_or_default();
        }
        titles.push(DlcTitle {
            title_id: group(1).to_lowercase(),
            version: group(2).parse().unwrap_or(0),
            stub,
            source: rest,
        });
    }
    titles
}

/// "0100152000023001 v65536" (upper case), as the status lines say it.
pub fn title_label(t: &DlcTitle) -> String {
    let version = if t.version != 0 {
        format!(" v{}", t.version)
    } else {
        String::new()
    };
    format!("{}{version}", t.title_id.to_uppercase())
}

/// "Booster Course.nsp — 0100152000023001 v65536" for the lists; `exists` says whether the package is
/// still there, `built` whether the game is.
pub fn description(package: &str, installed: &[DlcTitle], built: bool, exists: bool) -> String {
    let titles: Vec<String> = installed
        .iter()
        .filter(|t| same_path(&t.source, package))
        .map(|t| format!("{}{}", title_label(t), if t.stub { w::NO_DATA } else { "" }))
        .collect();
    let name = file_name(package);
    if !titles.is_empty() {
        return format!("{name} — {}", titles.join(", "));
    }
    if !exists {
        return format!("{name}{}", w::FILE_NOT_FOUND);
    }
    format!(
        "{name}{}",
        if built {
            w::NOT_INSTALLED_YET
        } else {
            w::WHEN_BUILT
        }
    )
}

/// Every package of `dlc` is installed and nothing else is.
pub fn up_to_date(dlc: &[String], installed: &[DlcTitle]) -> bool {
    dlc.iter()
        .all(|p| installed.iter().any(|t| same_path(&t.source, p)))
        && installed
            .iter()
            .all(|t| dlc.iter().any(|p| same_path(&t.source, p)))
}

/// Packages installed but missing from `dlc` join it. True when it changed.
pub fn adopt(dlc: &mut Vec<String>, installed: &[DlcTitle]) -> bool {
    let mut changed = false;
    for t in installed {
        if !t.source.is_empty() && !dlc.iter().any(|p| same_path(p, &t.source)) {
            dlc.push(super::path_text::from_native(&t.source));
            changed = true;
        }
    }
    changed
}

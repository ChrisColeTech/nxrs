//! nxrs-build: where it is, which launcher contract it answers, what its runs are given.

use super::data_dir::{app_dir, env};
use super::files::{is_dir, is_file, read_text};
use super::run::run_captured;
use crate::constants::{engine as e, timing, words_build as w};
use crate::model::fill::fill;
use crate::model::path_text::{absolute, join, native, parent};
use std::sync::LazyLock;
use std::time::Duration;

static CONTRACT: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"nxrs-contract (\d+)").ok());

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Toolchain {
    pub nxrs_build: String,
    pub nxrs_version: String,
    pub nxrs_contract: i64,
    pub keys: String,
}

/// A file, or the nxrs-build inside a folder; empty when neither.
fn resolve(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let p = absolute(path);
    if is_dir(&p) {
        let inside = join(
            &p,
            &format!("{}{}", e::NXRS_BUILD_NAME, e::EXECUTABLE_SUFFIX),
        );
        return if is_file(&inside) {
            inside
        } else {
            String::new()
        };
    }
    if is_file(&p) { p } else { String::new() }
}

impl Toolchain {
    /// `NXRS_BUILD`, else the one chosen in Settings, else the release's `nxrs-engine/` beside the
    /// launcher; then its contract from `engine.json` beside it, else from `nxrs-build contract`.
    pub fn find(chosen: &str, keys: &str) -> Toolchain {
        let mut t = Toolchain {
            keys: keys.to_string(),
            ..Toolchain::default()
        };
        let bundled = join(&app_dir(), e::BUNDLED_FOLDER);
        t.nxrs_build = [env(e::ENV_NXRS_BUILD), chosen.to_string(), bundled]
            .iter()
            .map(|c| resolve(c))
            .find(|p| !p.is_empty())
            .unwrap_or_default();
        if t.nxrs_build.is_empty() {
            return t;
        }
        let manifest = read_text(&join(&parent(&t.nxrs_build), e::MANIFEST))
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        if let Some(m) = manifest {
            t.nxrs_contract = m
                .get("contract")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            t.nxrs_version = m
                .get("version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            if t.nxrs_contract > 0 {
                return t;
            }
        }
        let asked = run_captured(
            &t.nxrs_build,
            &["contract".into()],
            &t.environment(),
            Duration::from_millis(timing::CONTRACT_MS),
        );
        if let Some((_, out)) = asked {
            t.nxrs_contract = CONTRACT
                .as_ref()
                .and_then(|re| re.captures(&out))
                .and_then(|m| m.get(1))
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(0);
        }
        t
    }

    /// Why nxrs cannot build here; empty when it can (Windows and Linux).
    pub fn problem(&self) -> String {
        if self.nxrs_build.is_empty() {
            return w::NOT_FOUND.into();
        }
        if self.nxrs_contract < e::CONTRACT_NEEDED {
            let (have, need) = (
                self.nxrs_contract.to_string(),
                e::CONTRACT_NEEDED.to_string(),
            );
            return fill(w::OLD_CONTRACT, &[&native(&self.nxrs_build), &have, &need]);
        }
        String::new()
    }
    pub fn can_build(&self) -> bool {
        self.problem().is_empty()
    }
    /// The installed keys exist (they are passed as `--keys` then).
    pub fn keys_exist(&self) -> bool {
        is_file(&self.keys)
    }
    /// The launcher's environment plus `SWR_PROD_KEYS`.
    pub fn environment(&self) -> Vec<(String, String)> {
        if self.keys.is_empty() {
            Vec::new()
        } else {
            vec![(e::ENV_KEYS.into(), self.keys.clone())]
        }
    }
    /// For Settings and the engine badge: nxrs-build's path, contract and version; the keys.
    pub fn health_text(&self) -> String {
        let nxrs = if self.nxrs_build.is_empty() {
            w::MISSING.to_string()
        } else {
            let version = if self.nxrs_version.is_empty() {
                String::new()
            } else {
                format!(", {}", self.nxrs_version)
            };
            format!(
                "{} (contract {}{version})",
                native(&self.nxrs_build),
                self.nxrs_contract
            )
        };
        let keys = if self.keys.is_empty() {
            w::MISSING.to_string()
        } else {
            native(&self.keys)
        };
        format!("Engine: {nxrs}\nKeys: {keys}")
    }
}

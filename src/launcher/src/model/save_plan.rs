//! Which of a Turbo build's save folders go where in its nxrs build. Turbo kept one folder per save type
//! (`user`, before the split `system`; `device`; `cache`), nxrs one per account (`user1`), `device`,
//! `cache0`; temporary data and BCAT are not copied.

use super::path_text::join;

#[derive(Clone, Debug, PartialEq)]
pub struct SaveFolderPair {
    pub from: String,
    pub to: String,
}

/// The plan, given which folders the Turbo save folder has (`has(name)`).
pub fn plan(from_save: &str, to_save: &str, has: impl Fn(&str) -> bool) -> Vec<SaveFolderPair> {
    let pair = |a: &str, b: &str| SaveFolderPair {
        from: join(from_save, a),
        to: join(to_save, b),
    };
    let mut plan = Vec::new();
    if has("user") {
        plan.push(pair("user", "user1"));
    } else if has("system") {
        plan.push(pair("system", "user1"));
    }
    if has("device") {
        plan.push(pair("device", "device"));
    }
    if has("cache") {
        plan.push(pair("cache", "cache0"));
    }
    plan
}

/// `<build folder>/<name>_data/save`.
pub fn save_folder(build_dir: &str, output_name: &str) -> String {
    join(&join(build_dir, &format!("{output_name}_data")), "save")
}

/// nxrs's transaction folders beside a save folder while a game runs.
pub const STALE: [&str; 3] = [".work", ".new", ".old"];

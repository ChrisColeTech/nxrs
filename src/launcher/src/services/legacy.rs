//! Earlier releases kept the library and keys under RecompLauncher/RecompLauncher and, before that,
//! RecompLauncher/SwitchRecompLauncher. The first of those that exists is copied once into the data
//! folder when it has no library yet; the old folders are left untouched.

use super::files::{exists, is_dir, list};
use crate::constants::engine as e;
use crate::model::path_text::{join, parent};

pub fn migrate(data_dir: &str) {
    if exists(&join(data_dir, e::LIBRARY_FILE)) {
        return;
    }
    let base = parent(data_dir);
    if let Some(legacy) = e::LEGACY_FOLDERS
        .iter()
        .map(|old| join(&base, old))
        .find(|p| is_dir(p))
    {
        copy_missing(&legacy, data_dir);
    }
}

fn copy_missing(from: &str, to: &str) {
    for (name, dir) in list(from) {
        let (source, target) = (join(from, &name), join(to, &name));
        if dir {
            copy_missing(&source, &target);
        } else if !exists(&target) {
            let _ = std::fs::create_dir_all(to);
            let _ = std::fs::copy(&source, &target);
        }
    }
}

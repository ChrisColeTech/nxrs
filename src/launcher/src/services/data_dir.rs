//! Where the launcher keeps its data and where it runs from.

use crate::constants::engine as e;
use crate::model::path_text::{absolute, from_native, join};

/// `NXRS_DATA_DIR` when set (development, tests), else `%LOCALAPPDATA%\nxrs` (`~/.local/share/nxrs`):
/// the library and the installed keys.
pub fn data_dir() -> String {
    if let Ok(dir) = std::env::var(e::ENV_DATA_DIR)
        && !dir.trim().is_empty()
    {
        return absolute(dir.trim());
    }
    let base = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    from_native(&base.join(e::DATA_FOLDER).to_string_lossy())
}

pub fn library_path() -> String {
    join(&data_dir(), e::LIBRARY_FILE)
}

pub fn installed_keys() -> String {
    join(&data_dir(), e::KEYS_FILE)
}

/// The running launcher's folder.
pub fn app_dir() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| from_native(&p.to_string_lossy())))
        .unwrap_or_else(|| ".".into())
}

/// An environment variable, empty when unset.
pub fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

//! Installing prod.keys into the launcher's data folder (`installKeys`).

use super::files::{read_bytes, write_atomic};
use crate::constants::{engine as e, words_library as w};
use crate::model::path_text::{join, parent};

/// Copies a key file to `<data>/keys/prod.keys` (atomic). Err: (title, text) for the message.
pub fn install(source: &str, data_dir: &str) -> Result<String, (String, String)> {
    let data = read_bytes(source).map_err(|err| (w::KEYS_FAILED.to_string(), err))?;
    let text = String::from_utf8_lossy(&data);
    if !text.contains("header_key") && !text.contains("header_key_source") {
        return Err((w::KEYS_INVALID_TITLE.into(), w::KEYS_INVALID.into()));
    }
    let dest = join(data_dir, e::KEYS_FILE);
    if std::fs::create_dir_all(parent(&dest)).is_err() {
        return Err((w::KEYS_FAILED.into(), w::KEYS_NO_FOLDER.into()));
    }
    write_atomic(&dest, &data).map_err(|err| (w::KEYS_FAILED.to_string(), err))?;
    Ok(dest)
}

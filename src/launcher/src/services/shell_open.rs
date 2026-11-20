//! Opening a folder or a file with the system's program (`QDesktopServices::openUrl`).

use crate::model::path_text::native;

pub fn open(path: &str) {
    if !path.is_empty() {
        let _ = open::that_detached(native(path));
    }
}

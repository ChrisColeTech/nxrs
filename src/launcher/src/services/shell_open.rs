//! Opening a folder or a file with the system's program (`QDesktopServices::openUrl`).

use crate::model::path_text::native;

pub fn open(path: &str) {
    if !path.is_empty() {
        let _ = open::that_detached(native(path));
    }
}

/// A web page in the default browser (the address as given: no path rewriting).
pub fn url(address: &str) {
    if !address.is_empty() {
        let _ = open::that_detached(address);
    }
}

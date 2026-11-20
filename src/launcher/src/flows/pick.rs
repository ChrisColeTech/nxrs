//! A path field's browse button: the system's picker, opened at the field's folder.

use crate::model::path_text::{from_native, parent};
use crate::services::files;
use crate::services::pickers::{self, Pick};

/// What the browse button picks.
pub enum Browse<'a> {
    File(Option<(&'a str, &'a [&'a str])>),
    Folder,
}

/// Several files (Add DLC…, Add update…), the picker opened in `start`.
pub fn pick_files(title: &str, start: &str, filter: (&str, &[&str])) -> Vec<String> {
    pickers::files(&Pick {
        title,
        filter: Some(filter),
        start,
    })
}

pub fn pick(title: &str, current: &str, browse: Browse) -> Option<String> {
    let now = from_native(current.trim());
    let start = if files::is_dir(&now) {
        now
    } else if now.is_empty() {
        String::new()
    } else {
        parent(&now)
    };
    match browse {
        Browse::Folder => pickers::folder(&Pick {
            title,
            filter: None,
            start: &start,
        }),
        Browse::File(filter) => pickers::file(&Pick {
            title,
            filter,
            start: &start,
        }),
    }
}

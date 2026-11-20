//! The system's file and folder pickers (the only dialogs not drawn by the launcher).

use crate::model::path_text::{from_native, native};

pub struct Pick<'a> {
    pub title: &'a str,
    /// (name, suffixes); "All files" is added after it
    pub filter: Option<(&'a str, &'a [&'a str])>,
    /// the folder it opens in
    pub start: &'a str,
}

fn dialog(p: &Pick) -> rfd::FileDialog {
    let mut d = rfd::FileDialog::new().set_title(p.title);
    if !p.start.is_empty() && std::path::Path::new(p.start).is_dir() {
        d = d.set_directory(native(p.start));
    }
    if let Some((name, exts)) = p.filter {
        d = d.add_filter(name, exts).add_filter("All files", &["*"]);
    }
    d
}

fn text(p: std::path::PathBuf) -> String {
    from_native(&p.to_string_lossy())
}

pub fn file(p: &Pick) -> Option<String> {
    dialog(p).pick_file().map(text)
}
pub fn files(p: &Pick) -> Vec<String> {
    dialog(p)
        .pick_files()
        .unwrap_or_default()
        .into_iter()
        .map(text)
        .collect()
}
pub fn folder(p: &Pick) -> Option<String> {
    dialog(p).pick_folder().map(text)
}

//! What each command does (the Qt `Window`'s slots), over the state in `state`. No egui: the UI draws
//! from this state and turns input into `Action`s; command-line verbs and tests drive it directly.
mod action;
mod add;
mod artwork;
mod build;
mod build_finish;
mod dispatch;
mod dlc;
mod dlc_sync;
mod migrate;
mod open;
mod pick;
mod play;
pub mod properties;
mod properties_apply;
mod queries;
mod runs;
mod saves;
mod selection;
pub mod settings;
mod start;
mod state;
pub mod surface;
mod target;
mod tick;
mod tools;
mod update_probe;
mod updates;
pub mod view_state;
mod web;
mod web_import;

/// Covers and backgrounds for the gallery's image cache.
pub use crate::services::artwork_decode::decode as decode_artwork;
pub use crate::services::files::stamp as file_stamp;
pub use action::{Action, DlcReason, Outcome};
pub use pick::{Browse, pick, pick_files};

/// `--screenshot`: an RGBA frame written as PNG, at most `max` wide.
pub fn write_png(path: &str, (w, h): (u32, u32), rgba: Vec<u8>, max: u32) -> Result<(), String> {
    crate::services::artwork_decode::write_png(path, w, h, rgba, max)
}
pub use state::App;

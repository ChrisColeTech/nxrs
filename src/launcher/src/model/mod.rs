//! The launcher's data and the rules over it, with no I/O: games, the library, statuses, filters, the
//! engine's arguments and answers, INI text, the library file's JSON.
pub mod args;
pub mod build_layout;
pub mod card_state;
pub mod dates;
pub mod dlc_title;
pub mod doctor;
pub mod facts;
pub mod fill;
pub mod filter;
pub mod game;
pub mod game_json;
pub mod game_update;
pub mod http_cache;
pub mod http_range;
pub mod ini_text;
pub mod json_fields;
pub mod library;
pub mod library_json;
pub mod names;
pub mod output_lines;
pub mod path_text;
pub mod play;
pub mod qtjson;
pub mod shader_xlate;
pub mod update_text;
pub mod web_build;
pub mod ws;

#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod web_tests;
#[cfg(test)]
mod ws_tests;

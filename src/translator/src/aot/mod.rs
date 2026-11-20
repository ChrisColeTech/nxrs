//! Ahead-of-time generation: function discovery over a module's .text, chunked parallel emission of one C function
//! per guest function, function tables, layout and patch headers.

pub mod discover;
pub mod emit;
pub mod gen;
pub mod nso_match;
pub mod patches;
pub mod runtime_headers;

pub use gen::{generate, GenOptions, GenReport};

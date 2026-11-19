//! The RecompLauncher engine's translator: Switch package input and ARM64 -> C translation, in one crate.
//!
//! | Module | What |
//! |---|---|
//! | [`package`] | folder / NSP / NUP / XCI / NCA, keys; ExeFS, RomFS read in place, updates (BKTR) and DLC |
//! | [`loader`] | NSO image, MOD0, symbols, relocations, function seeds, guest layout |
//! | [`xlate`] | AArch64 -> C per instruction, NEON/FP, local flags and registers, jump tables |
//! | [`aot`] | function discovery, chunked parallel emission, tables, patch table, runtime headers |
//!
//! The binary `nxrs` (src/bin/nxrs.rs) drives them. Credits and licenses: the repository's README.md.

pub mod aot;
pub mod loader;
pub mod package;
pub mod xlate;

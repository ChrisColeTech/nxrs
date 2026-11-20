//! Everything that touches the disk, other processes or the OS. The model decides; these read, write,
//! spawn and watch.
pub mod artwork_decode;
pub mod build_facts;
pub mod data_dir;
pub mod dlc_files;
pub mod files;
pub mod font_files;
pub mod ini_file;
pub mod jobs;
pub mod keys;
pub mod legacy;
pub mod library_file;
pub mod library_ops;
pub mod markers;
pub mod pickers;
pub mod platform;
pub mod run;
pub mod saves;
pub mod shell_open;
pub mod sources;
pub mod toolchain;
pub mod update_files;

#[cfg(test)]
mod dlc_tests;
#[cfg(test)]
mod ini_tests;
#[cfg(test)]
mod library_tests;
#[cfg(test)]
mod services_tests;
#[cfg(test)]
mod sources_tests;

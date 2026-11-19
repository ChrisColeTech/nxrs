//! Error type for the package readers.
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{0}")]
    Format(String),
    #[error("keys: {0}")]
    Keys(String),
    #[error("{0}: not a recognised package (folder with ExeFS, .nsp, .xci or .nup)")]
    Unrecognised(PathBuf),
}

pub type Result<T> = std::result::Result<T, PackageError>;

pub(crate) fn fmt<T>(msg: impl Into<String>) -> Result<T> {
    Err(PackageError::Format(msg.into()))
}

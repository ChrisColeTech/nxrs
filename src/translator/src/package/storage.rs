//! A readable byte storage: an NCA section, the BKTR-patched section of an update, a plain file range.
//! Every storage is `Send + Sync`: the runtime's file system calls `read` from any thread.
use crate::package::error::Result;
use crate::package::io::Blob;
use std::sync::Arc;

pub trait Storage: Send + Sync {
    fn size(&self) -> u64;
    /// `size` bytes at `offset`; reading past the end is an error.
    fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>>;
}

/// A plain byte range of a file (a plaintext RomFS section of an NSP).
pub struct FileRange {
    pub blob: Arc<Blob>,
    pub offset: u64,
    pub size: u64,
}

impl Storage for FileRange {
    fn size(&self) -> u64 {
        self.size
    }
    fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        if offset.checked_add(size).map_or(true, |e| e > self.size) {
            return crate::package::error::fmt(format!("{}: read past the end of the RomFS", self.blob.path.display()));
        }
        self.blob.read_at(self.offset + offset, size)
    }
}

/// A sub-range of another storage (the RomFS image inside its section).
pub struct Slice {
    pub inner: Arc<dyn Storage>,
    pub offset: u64,
    pub size: u64,
}

impl Storage for Slice {
    fn size(&self) -> u64 {
        self.size
    }
    fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        if offset.checked_add(size).map_or(true, |e| e > self.size) {
            return crate::package::error::fmt("read past the end of the RomFS image");
        }
        self.inner.read(self.offset + offset, size)
    }
}

/// In-memory storage (tests and small images).
impl Storage for Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
    }
    fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        match self.get(offset as usize..(offset + size) as usize) {
            Some(s) => Ok(s.to_vec()),
            None => crate::package::error::fmt("read past the end of the storage"),
        }
    }
}

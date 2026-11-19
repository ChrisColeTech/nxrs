//! Ranged reads from multi-GB packages. Reads are positional (no shared cursor), so one [`Blob`] can be shared by
//! every reader thread of the runtime (`Arc<Blob>`).
use crate::package::error::{PackageError, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Blob {
    pub path: PathBuf,
    pub size: u64,
    file: File,
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blob({}, {} bytes)", self.path.display(), self.size)
    }
}

#[cfg(windows)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

#[cfg(unix)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

impl Blob {
    pub fn open(path: &Path) -> Result<Blob> {
        let io = |source| PackageError::Io { path: path.to_path_buf(), source };
        let file = File::open(path).map_err(io)?;
        let size = file.metadata().map_err(io)?.len();
        Ok(Blob { path: path.to_path_buf(), size, file })
    }

    /// Fill `buf` from `offset`; a short file is an error ("data at ... is cut short", as nsp.py `_read`).
    pub fn read_into(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset.checked_add(buf.len() as u64).map_or(true, |end| end > self.size) {
            return crate::package::error::fmt(format!("{}: data at {offset:#x} is cut short", self.path.display()));
        }
        let mut done = 0;
        while done < buf.len() {
            let n = pread(&self.file, &mut buf[done..], offset + done as u64)
                .map_err(|source| PackageError::Io { path: self.path.clone(), source })?;
            if n == 0 {
                return crate::package::error::fmt(format!("{}: data at {offset:#x} is cut short", self.path.display()));
            }
            done += n;
        }
        Ok(())
    }

    pub fn read_at(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; size as usize];
        self.read_into(offset, &mut buf)?;
        Ok(buf)
    }

    /// A reader positioned at `offset`, limited to `size` bytes (for streaming decoders).
    pub fn reader(&self, offset: u64, size: u64) -> Result<std::io::Take<File>> {
        use std::io::{Read, Seek, SeekFrom};
        let io = |source| PackageError::Io { path: self.path.clone(), source };
        let mut file = File::open(&self.path).map_err(io)?;
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        Ok(file.take(size))
    }
}

pub fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
pub fn u64le(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// Python's `os.path.abspath`: absolute and normalised, without resolving links (on Windows `/` becomes `\`).
pub fn abspath(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
pub fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}

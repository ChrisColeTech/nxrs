//! The file a web answer sends: the build's precompressed copy when the request takes it, its
//! modification time for the ETag, and the bytes copied to the connection.

use crate::constants::web as c;
use crate::model::http_cache;
use std::fs::File;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

/// A file's modification time in nanoseconds since 1970.
pub fn mtime(m: &std::fs::Metadata) -> Option<u128> {
    let t = m.modified().ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos())
}

/// The build's gzip copy (`<file>.gz`) instead of `f` when the request takes it; whether it was.
pub fn pick_encoding(
    range: Option<&str>,
    accept: Option<&str>,
    path: &Path,
    f: File,
) -> std::io::Result<(File, bool)> {
    let mut gz = path.as_os_str().to_owned();
    gz.push(c::GZIP_SUFFIX);
    let gz = PathBuf::from(gz);
    let gz_time = std::fs::metadata(&gz)
        .ok()
        .filter(|m| m.is_file())
        .and_then(|m| mtime(&m));
    let file_time = mtime(&f.metadata()?).unwrap_or(0);
    let open = http_cache::use_gzip(range, accept, file_time, gz_time)
        .then(|| File::open(&gz).ok())
        .flatten();
    Ok(match open {
        Some(z) => (z, true),
        None => (f, false),
    })
}

/// `n` bytes of the file to the connection, a chunk a write.
pub fn copy(f: &mut File, out: &mut TcpStream, mut n: u64) -> std::io::Result<()> {
    let mut buf = vec![0u8; c::CHUNK];
    while n > 0 {
        let want = usize::try_from(n).map_or(buf.len(), |n| n.min(buf.len()));
        let got = f.read(&mut buf[..want])?;
        if got == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        out.write_all(&buf[..got])?;
        n -= got as u64;
    }
    out.flush()
}

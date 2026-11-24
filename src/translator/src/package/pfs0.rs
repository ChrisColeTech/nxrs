//! PartitionFS (PFS0) parsing.
use crate::package::error::{fmt, Result};
use crate::package::io::{u32le, u64le};

const HEAD: u64 = 0x10;
const ENTRY: u64 = 0x18;
const MAX_TABLE: u64 = 256 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Offset of the file data relative to the start of the PFS0 image.
    pub offset: u64,
    pub size: u64,
}

/// Parse a PFS0 of `size` bytes; `read(offset, len)` returns bytes relative to the PFS0 start.
pub fn parse(
    read: &dyn Fn(u64, u64) -> Result<Vec<u8>>,
    size: u64,
    what: &str,
) -> Result<Vec<Entry>> {
    if size < HEAD {
        return fmt(format!("{what}: PFS0 is too small"));
    }
    let head = read(0, HEAD)?;
    let count = u32le(&head, 4) as u64;
    let strings = u32le(&head, 8) as u64;
    if &head[..4] != b"PFS0" {
        return fmt(format!("{what}: no PFS0 magic (wrong keys or not a PFS0)"));
    }
    let table_size = HEAD + count * ENTRY + strings;
    if count > 100_000 || strings > MAX_TABLE || table_size > size {
        return fmt(format!("{what}: invalid PFS0 table"));
    }
    let table = read(0, table_size)?;
    parse_table(&table, size, what)
}

/// Parse a PFS0 whose whole header table is already in `table`.
pub fn parse_table(table: &[u8], size: u64, what: &str) -> Result<Vec<Entry>> {
    if table.len() < HEAD as usize || &table[..4] != b"PFS0" {
        return fmt(format!("{what}: no PFS0 magic"));
    }
    let count = u32le(table, 4) as u64;
    let strings_size = u32le(table, 8) as u64;
    let table_size = HEAD + count * ENTRY + strings_size;
    if count > 100_000 || table_size > table.len() as u64 || table_size > size {
        return fmt(format!("{what}: invalid PFS0 table"));
    }
    let strings = &table[(HEAD + count * ENTRY) as usize..table_size as usize];
    let mut out: Vec<Entry> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let at = (HEAD + i * ENTRY) as usize;
        let rel = u64le(table, at);
        let len = u64le(table, at + 8);
        let name_at = u32le(table, at + 16) as usize;
        if name_at >= strings.len() {
            return fmt(format!("{what}: PFS0 name out of range"));
        }
        let end = match strings[name_at..].iter().position(|&b| b == 0) {
            Some(p) => name_at + p,
            None => return fmt(format!("{what}: unterminated PFS0 name")),
        };
        let name = std::str::from_utf8(&strings[name_at..end])
            .map_err(|_| {
                crate::package::error::PackageError::Format(format!("{what}: non-UTF-8 PFS0 name"))
            })?
            .to_string();
        let room = size - table_size;
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || rel > room
            || len > room - rel
            || out.iter().any(|e| e.name == name)
        {
            return fmt(format!("{what}: unsafe PFS0 entry {name:?}"));
        }
        out.push(Entry {
            name,
            offset: table_size + rel,
            size: len,
        });
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut strings = Vec::new();
    let mut entries = Vec::new();
    let mut data = Vec::new();
    for (name, bytes) in files {
        entries.extend_from_slice(&(data.len() as u64).to_le_bytes());
        entries.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        entries.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        entries.extend_from_slice(&0u32.to_le_bytes());
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
        data.extend_from_slice(bytes);
    }
    while strings.len() % 0x20 != 0 {
        strings.push(0);
    }
    let mut out = b"PFS0".to_vec();
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend(entries);
    out.extend(strings);
    out.extend(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_pfs0() {
        let img = build(&[("main", b"NSO0hello"), ("main.npdm", b"META"), ("sdk", b"")]);
        let size = img.len() as u64;
        let read = |o: u64, n: u64| Ok(img[o as usize..(o + n) as usize].to_vec());
        let e = parse(&read, size, "test").unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].name, "main");
        assert_eq!(
            &img[e[0].offset as usize..(e[0].offset + e[0].size) as usize],
            b"NSO0hello"
        );
        assert_eq!(
            &img[e[1].offset as usize..(e[1].offset + e[1].size) as usize],
            b"META"
        );
        assert_eq!(e[2].size, 0);
    }

    #[test]
    fn rejects_bad_magic_and_overflow() {
        let mut img = build(&[("a", b"x")]);
        let read = |o: u64, n: u64| Ok(img[o as usize..(o + n) as usize].to_vec());
        assert!(parse(&read, img.len() as u64 - 1, "t").is_err()); // data past the end
        img[0] = b'X';
        let read = |o: u64, n: u64| Ok(img[o as usize..(o + n) as usize].to_vec());
        assert!(parse(&read, img.len() as u64, "t").is_err());
    }
}

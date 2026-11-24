//! The members of a package file: an NSP / NSZ (PFS0), a card image XCI / XCZ (the `secure` HFS0 partition, with
//! or without the 0x1000-byte key area in front), or a bare NCA (its own only member).
use crate::package::error::{fmt, Result};
use crate::package::io::{u32le, u64le, Blob};
use crate::package::pfs0;

pub const EXTENSIONS: [&str; 5] = [".nsp", ".nsz", ".xci", ".xcz", ".nca"];
const MAX_TABLE: u64 = 256 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    /// Offset of the member in the package file.
    pub offset: u64,
    pub size: u64,
}

pub fn extension(path: &std::path::Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default()
}

fn hfs(blob: &Blob, offset: u64, limit: u64) -> Result<Vec<Member>> {
    let path = blob.path.display();
    let head = blob.read_at(offset, 0x10)?;
    let (count, strings_size) = (u32le(&head, 4) as u64, u32le(&head, 8) as u64);
    let table_size = 0x10 + count * 0x40 + strings_size;
    if &head[..4] != b"HFS0" || count > 100_000 || strings_size > MAX_TABLE || table_size > limit {
        return fmt(format!("{path}: invalid HFS0 at {offset:#x}"));
    }
    let table = blob.read_at(offset, table_size)?;
    let strings = &table[(0x10 + count * 0x40) as usize..];
    let mut out = vec![];
    for i in 0..count as usize {
        let at = 0x10 + i * 0x40;
        let (rel, len, name_at) = (
            u64le(&table, at),
            u64le(&table, at + 8),
            u32le(&table, at + 16) as usize,
        );
        let end = strings
            .get(name_at..)
            .and_then(|s| s.iter().position(|&b| b == 0))
            .map(|p| name_at + p);
        let Some(end) = end else {
            return fmt(format!("{path}: HFS0 entry is out of range"));
        };
        if rel > limit - table_size || len > limit - table_size - rel {
            return fmt(format!("{path}: HFS0 entry is out of range"));
        }
        out.push(Member {
            name: String::from_utf8_lossy(&strings[name_at..end]).into_owned(),
            offset: offset + table_size + rel,
            size: len,
        });
    }
    Ok(out)
}

/// The files of a package: name, offset and size inside it.
pub fn members(blob: &Blob) -> Result<Vec<Member>> {
    let path = blob.path.display();
    let ext = extension(&blob.path);
    if !EXTENSIONS.contains(&ext.as_str()) {
        return fmt(format!("{path}: not a package ({})", EXTENSIONS.join(" ")));
    }
    if ext == ".nca" {
        let name = blob
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Ok(vec![Member {
            name,
            offset: 0,
            size: blob.size,
        }]);
    }
    let head = if blob.size >= 0x10 {
        blob.read_at(0, blob.size.min(0x1200))?
    } else {
        vec![]
    };
    if head.get(..4) == Some(b"PFS0") {
        let read = |o: u64, n: u64| blob.read_at(o, n);
        return Ok(pfs0::parse(&read, blob.size, &path.to_string())?
            .into_iter()
            .map(|e| Member {
                name: e.name,
                offset: e.offset,
                size: e.size,
            })
            .collect());
    }
    for card in [0usize, 0x1000] {
        if head.get(card + 0x100..card + 0x104) == Some(b"HEAD") {
            let root_at = u64le(&head, card + 0x130);
            let start = card as u64 + root_at;
            for part in hfs(blob, start, blob.size.saturating_sub(start))? {
                if part.name == "secure" {
                    return hfs(blob, part.offset, part.size);
                }
            }
            return fmt(format!("{path}: the card image has no secure partition"));
        }
    }
    fmt(format!(
        "{path}: neither a PFS0 package (NSP, NSZ) nor a card image (XCI, XCZ)"
    ))
}

#[cfg(test)]
pub(crate) fn build_hfs(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut strings = vec![];
    let mut entries = vec![];
    let mut data = vec![];
    for (name, bytes) in files {
        let mut e = vec![0u8; 0x40];
        e[..8].copy_from_slice(&(data.len() as u64).to_le_bytes());
        e[8..16].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
        e[16..20].copy_from_slice(&(strings.len() as u32).to_le_bytes());
        entries.extend(e);
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
        data.extend_from_slice(bytes);
    }
    let mut out = b"HFS0".to_vec();
    out.extend((files.len() as u32).to_le_bytes());
    out.extend((strings.len() as u32).to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(entries);
    out.extend(strings);
    out.extend(data);
    out
}

/// A card image whose secure partition holds `files`; `key_area` puts the 0x1000-byte area in front.
#[cfg(test)]
pub(crate) fn build_xci(files: &[(&str, &[u8])], key_area: bool) -> Vec<u8> {
    let secure = build_hfs(files);
    let root = build_hfs(&[("update", b""), ("secure", &secure)]);
    let mut card = vec![0u8; 0x200];
    card[0x100..0x104].copy_from_slice(b"HEAD");
    card[0x130..0x138].copy_from_slice(&0x200u64.to_le_bytes());
    card.extend(root);
    if key_area {
        let mut out = vec![0u8; 0x1000];
        out.extend(card);
        out
    } else {
        card
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_image_with_and_without_key_area() {
        let dir = std::env::temp_dir().join(format!("nxrs-container-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for ka in [false, true] {
            let p = dir.join(format!("t{}.xci", ka as u8));
            std::fs::write(&p, build_xci(&[("a.nca", b"hello"), ("b.tik", b"tik")], ka)).unwrap();
            let blob = Blob::open(&p).unwrap();
            let m = members(&blob).unwrap();
            assert_eq!(
                m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
                ["a.nca", "b.tik"]
            );
            assert_eq!(blob.read_at(m[0].offset, m[0].size).unwrap(), b"hello");
        }
        let p = dir.join("x.bin");
        std::fs::write(&p, b"0000000000000000").unwrap();
        assert!(members(&Blob::open(&p).unwrap())
            .unwrap_err()
            .to_string()
            .contains("not a package (.nsp .nsz .xci .xcz .nca)"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

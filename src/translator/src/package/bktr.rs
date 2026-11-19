//! BKTR: an update's patch RomFS section composed over the base game's RomFS section, per read.
//!
//! ```text
//! table   = header(0x4000) + buckets * 0x4000
//! header  = u32 pad, u32 bucket count, u64 total size, u64 bucket start offsets[]
//! bucket  = u32 pad, u32 entry count, u64 end offset, entries[]
//! indirect entry (0x14) = u64 virtual offset, u64 physical offset, u32 source (0 = base section, 1 = patch)
//! AesCtrEx entry (0x10) = u64 offset in the patch section, u32 pad, u32 counter generation
//! ```
//! Both tables sit inside the patch section, encrypted with the section's own counter. A read of the virtual
//! (patched) section walks the indirect table (relocations); ranges of the patch are decrypted with the counter
//! generation the AesCtrEx table gives for them (the AES-CTR-EX layer under the indirect layer).
use crate::package::error::{fmt, Result};
use crate::package::io::{u32le, u64le};
use crate::package::nca::SectionStorage;
use crate::package::storage::Storage;
use std::sync::Arc;

pub const MAX_BKTR: u64 = 256 << 20;

pub struct Patched {
    pub patch: SectionStorage,
    pub base: Option<Arc<dyn Storage>>,
    /// (indirect offset, size, AesCtrEx offset, size) in the patch section.
    pub tables: (u64, u64, u64, u64),
    /// (virtual offset, physical offset, source)
    pub relocations: Vec<(u64, u64, u32)>,
    /// (offset in the patch section, counter generation)
    pub generations: Vec<(u64, u32)>,
    /// Size of the virtual (patched) section.
    pub size: u64,
}

/// The entries of a bucketed BKTR table: (first u64, second field) per entry, plus the header's total size.
fn buckets(table: &[u8], entry_size: usize, path: &str, ex: bool) -> Result<(Vec<(u64, u64, u32)>, u64)> {
    if table.len() < 0x4000 {
        return fmt(format!("{path}: a BKTR table is cut short"));
    }
    let count = u32le(table, 4) as usize;
    let total = u64le(table, 8);
    let mut entries = vec![];
    for bucket in 0..count {
        let at = 0x4000 + bucket * 0x4000;
        if at + 0x4000 > table.len() {
            return fmt(format!("{path}: a BKTR bucket is out of range"));
        }
        let n = u32le(table, at + 4) as usize;
        if n > 0x3ff0 / entry_size {
            return fmt(format!("{path}: a BKTR bucket is too large"));
        }
        for i in 0..n {
            let e = at + 0x10 + i * entry_size;
            if ex {
                // '<QII': offset, pad, generation
                entries.push((u64le(table, e), u32le(table, e + 8) as u64, u32le(table, e + 12)));
            } else {
                // '<QQI': virtual, physical, source
                entries.push((u64le(table, e), u64le(table, e + 8), u32le(table, e + 16)));
            }
        }
    }
    if entries.windows(2).any(|w| w[0].0 > w[1].0) {
        return fmt(format!("{path}: a BKTR table is not sorted"));
    }
    Ok((entries, total))
}

/// The index of the last entry whose first field is <= offset (0 when none is).
fn find<T>(entries: &[T], key: impl Fn(&T) -> u64, offset: u64) -> usize {
    let (mut low, mut high) = (0usize, entries.len().saturating_sub(1));
    while low < high {
        let middle = (low + high + 1) / 2;
        if key(&entries[middle]) <= offset {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

impl Patched {
    /// The patched RomFS section: `patch` is the update's RomFS section (type 4), `base` the game's RomFS section
    /// (None for an update without its game, which is refused with Turbo's reason).
    pub fn new(patch: SectionStorage, base: Option<Arc<dyn Storage>>) -> Result<Patched> {
        let path = patch.blob.path.display().to_string();
        let fs = &patch.fs;
        let (indirect_at, indirect_size) = (u64le(fs, 0x100), u64le(fs, 0x108));
        let (ex_at, ex_size) = (u64le(fs, 0x120), u64le(fs, 0x128));
        if &fs[0x110..0x114] != b"BKTR" || &fs[0x130..0x134] != b"BKTR" {
            return fmt(format!("{path}: the update has no BKTR tables"));
        }
        Self::from_tables(patch, base, (indirect_at, indirect_size, ex_at, ex_size))
    }

    /// The same from the table locations alone (the runtime marker's `patch` row).
    pub fn from_tables(patch: SectionStorage, base: Option<Arc<dyn Storage>>, tables: (u64, u64, u64, u64)) -> Result<Patched> {
        let path = patch.blob.path.display().to_string();
        let (indirect_at, indirect_size, ex_at, ex_size) = tables;
        let psize = patch.size();
        for (at, size) in [(indirect_at, indirect_size), (ex_at, ex_size)] {
            if size > MAX_BKTR || at > psize || size > psize - at {
                return fmt(format!("{path}: a BKTR table is out of range"));
            }
        }
        let (relocations, size) = buckets(&patch.read(indirect_at, indirect_size)?, 0x14, &path, false)?;
        let relocations: Vec<(u64, u64, u32)> = relocations;
        let (gens, _) = buckets(&patch.read(ex_at, ex_size)?, 0x10, &path, true)?;
        let generations: Vec<(u64, u32)> = gens.into_iter().map(|(at, _, g)| (at, g)).collect();
        if relocations.is_empty() || relocations[0].0 != 0 || generations.is_empty() {
            return fmt(format!(
                "{path}: the BKTR tables of the update are empty or do not start at 0 (wrong keys?)"
            ));
        }
        let base_size = base.as_ref().map_or(0, |b| b.size());
        for (i, &(virt, phys, source)) in relocations.iter().enumerate() {
            let end = relocations.get(i + 1).map_or(size, |r| r.0);
            let limit = if source != 0 { psize } else { base_size };
            if source > 1 || end < virt || phys > limit || end - virt > limit - phys {
                if source == 0 && base.is_none() {
                    return fmt(format!("{path}: this is an update without its game; use a package that holds both"));
                }
                return fmt(format!(
                    "{path}: the update does not fit the game in this package (BKTR range out of bounds)"
                ));
            }
        }
        Ok(Patched {
            patch,
            base,
            tables: (indirect_at, indirect_size, ex_at, ex_size),
            relocations,
            generations,
            size,
        })
    }

    /// The AES-CTR-EX layer: patch-section bytes, each range with its own counter generation.
    fn read_patch(&self, mut offset: u64, mut size: u64) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(size as usize);
        let mut index = find(&self.generations, |g| g.0, offset);
        let psize = self.patch.size();
        while size > 0 {
            let end = self.generations.get(index + 1).map_or(psize, |g| g.0);
            let part = size.min(end.saturating_sub(offset));
            if part == 0 {
                return fmt(format!("{}: a BKTR patch range is out of bounds", self.patch.blob.path.display()));
            }
            out.extend(self.patch.read_gen(offset, part, Some(self.generations[index].1))?);
            offset += part;
            size -= part;
            index += 1;
        }
        Ok(out)
    }
}

impl Storage for Patched {
    fn size(&self) -> u64 {
        self.size
    }

    /// The indirect layer: a read of the virtual section, composed from the base and the patch.
    fn read(&self, mut offset: u64, mut size: u64) -> Result<Vec<u8>> {
        if offset.checked_add(size).map_or(true, |e| e > self.size) {
            return fmt(format!("{}: read past the end of the patched RomFS", self.patch.blob.path.display()));
        }
        let mut out = Vec::with_capacity(size as usize);
        let mut index = find(&self.relocations, |r| r.0, offset);
        while size > 0 {
            let (virt, phys, source) = self.relocations[index];
            let end = self.relocations.get(index + 1).map_or(self.size, |r| r.0);
            let part = size.min(end - offset);
            let at = phys + offset - virt;
            if source != 0 {
                out.extend(self.read_patch(at, part)?);
            } else {
                out.extend(self.base.as_ref().expect("checked at open").read(at, part)?);
            }
            offset += part;
            size -= part;
            index += 1;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_is_last_not_greater() {
        let e = [0u64, 10, 20, 30];
        assert_eq!(find(&e, |x| *x, 0), 0);
        assert_eq!(find(&e, |x| *x, 9), 0);
        assert_eq!(find(&e, |x| *x, 10), 1);
        assert_eq!(find(&e, |x| *x, 35), 3);
        assert_eq!(find(&[5u64], |x| *x, 0), 0);
    }

    #[test]
    fn bucket_parse_and_sorting() {
        let mut t = vec![0u8; 0x8000];
        t[4..8].copy_from_slice(&1u32.to_le_bytes());
        t[8..16].copy_from_slice(&0x300u64.to_le_bytes());
        t[0x4004..0x4008].copy_from_slice(&2u32.to_le_bytes());
        for (i, (v, p, s)) in [(0u64, 0x10u64, 0u32), (0x100, 0x20, 1)].iter().enumerate() {
            let e = 0x4010 + i * 0x14;
            t[e..e + 8].copy_from_slice(&v.to_le_bytes());
            t[e + 8..e + 16].copy_from_slice(&p.to_le_bytes());
            t[e + 16..e + 20].copy_from_slice(&s.to_le_bytes());
        }
        let (e, total) = buckets(&t, 0x14, "t", false).unwrap();
        assert_eq!(total, 0x300);
        assert_eq!(e, vec![(0, 0x10, 0), (0x100, 0x20, 1)]);
        t[0x4010..0x4018].copy_from_slice(&0x200u64.to_le_bytes());
        assert!(buckets(&t, 0x14, "t", false).unwrap_err().to_string().contains("not sorted"));
        assert!(buckets(&t[..0x100], 0x14, "t", false).unwrap_err().to_string().contains("cut short"));
    }
}

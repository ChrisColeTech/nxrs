//! Compressed NCA storage (the FS header's CompressionInfo at +0x178): a section whose data level is stored in LZ4
//! blocks, zero runs and plain runs, described by a bucket tree stored after the data. Card images (XCI) of newer
//! games hold their Program NCA this way (Super Mario Party Jamboree).
//!
//! ```text
//! FS header +0x178  u64 table offset, u64 table size (from the data level's start), BKTR header (magic, version,
//!                   entry count)
//! section           [hash levels, stored as they are][data level: compressed blocks, then the table]
//! table             header node (0x4000): u32 index, u32 entry set count, u64 decompressed size, u64 set offsets[]
//!                   entry sets (0x4000 each): u32 index, u32 entry count, u64 end offset, entries[]
//! entry (0x18)      u64 virtual offset, u64 physical offset, u8 type (0 plain, 1 zeros, 3 LZ4), i8 level,
//!                   u16 pad, u32 physical size
//! ```
//! The data level is the IVFC header's last level (RomFS) or the SHA-256 header's data range (PFS0); its offset in
//! the FS header stays valid, its size there is the stored size (the hash levels cover the stored, compressed bytes)
//! and the table's is the decompressed size. The view this storage gives is the section with its data level
//! decompressed: the hash levels pass through, the data level is read through the table per block, and decoded
//! blocks are kept in a small cache.
use crate::package::error::{fmt, PackageError, Result};
use crate::package::io::{u32le, u64le};
use crate::package::storage::Storage;
use std::sync::{Arc, Mutex};

const NODE: usize = 0x4000;
const ENTRY: usize = 0x18;
const MAX_TABLE: u64 = 256 << 20;
const CACHE: usize = 32;

#[derive(Debug, Clone, Copy)]
struct Entry {
    virt: u64,
    phys: u64,
    kind: u8,
    phys_size: u32,
}

pub struct Compressed {
    inner: Arc<dyn Storage>,
    /// Where the data level starts in `inner` (and in this view).
    head: u64,
    entries: Vec<Entry>,
    /// Decompressed size of the data level.
    data_size: u64,
    cache: Mutex<Vec<(usize, Arc<Vec<u8>>)>>,
    what: String,
}

/// (table offset, table size) from an FS header, when the section has a compression layer.
pub fn table_of(fs: &[u8]) -> Option<(u64, u64)> {
    let c = fs.get(0x178..0x1a0)?;
    if u64le(c, 8) == 0 || &c[0x10..0x14] != b"BKTR" {
        return None;
    }
    Some((u64le(c, 0), u64le(c, 8)))
}

/// (offset, size) of a section's data level as its FS header gives it: the IVFC header's last level (hash type 3)
/// or the SHA-256 header's data range (hash type 2).
pub fn data_level(fs: &[u8]) -> Option<(u64, u64)> {
    match fs.get(3)? {
        3 if fs.get(8..12) == Some(b"IVFC") => {
            let levels = u32le(fs, 0x14) as usize;
            if !(2..=7).contains(&levels) {
                return None;
            }
            let at = 0x18 + (levels - 2) * 0x18;
            Some((u64le(fs, at), u64le(fs, at + 8)))
        }
        2 => Some((u64le(fs, 0x40), u64le(fs, 0x48))),
        _ => None,
    }
}

/// The section `inner` (whose FS header is `fs`) with its data level decompressed, when it has a compression layer:
/// (the view, (data level offset, table offset, table size)).
pub fn over(
    inner: Arc<dyn Storage>,
    fs: &[u8],
    what: &str,
) -> Result<Option<(Compressed, (u64, u64, u64))>> {
    let Some(table) = table_of(fs) else {
        return Ok(None);
    };
    let Some((head, _)) = data_level(fs) else {
        return fmt(format!("{what}: a compressed section without a data level"));
    };
    Ok(Some((
        Compressed::new(inner, head, table, what)?,
        (head, table.0, table.1),
    )))
}

impl Compressed {
    /// The section `inner` with its data level (at `head`) decompressed through the table at `table` (relative to
    /// `head`).
    pub fn new(
        inner: Arc<dyn Storage>,
        head: u64,
        table: (u64, u64),
        what: &str,
    ) -> Result<Compressed> {
        let (at, size) = table;
        let room = inner.size().saturating_sub(head);
        if size > MAX_TABLE || size < NODE as u64 || at > room || size > room - at {
            return fmt(format!("{what}: the compression table is out of range"));
        }
        let t = inner.read(head + at, size)?;
        let sets = u32le(&t, 4) as usize;
        let total = u64le(&t, 8);
        if sets == 0 || NODE * (1 + sets) > t.len() {
            return fmt(format!(
                "{what}: the compression table is damaged (wrong keys?)"
            ));
        }
        let mut entries = vec![];
        for set in 0..sets {
            let base = NODE * (1 + set);
            let n = u32le(&t, base + 4) as usize;
            if n > (NODE - 0x10) / ENTRY {
                return fmt(format!(
                    "{what}: a compression table entry set is too large"
                ));
            }
            for i in 0..n {
                let e = base + 0x10 + i * ENTRY;
                entries.push(Entry {
                    virt: u64le(&t, e),
                    phys: u64le(&t, e + 8),
                    kind: t[e + 16],
                    phys_size: u32le(&t, e + 20),
                });
            }
        }
        if entries.is_empty()
            || entries[0].virt != 0
            || entries.windows(2).any(|w| w[0].virt >= w[1].virt)
            || entries.last().unwrap().virt >= total
        {
            return fmt(format!(
                "{what}: the compression table is damaged (wrong keys?)"
            ));
        }
        for (i, e) in entries.iter().enumerate() {
            let len = entries.get(i + 1).map_or(total, |n| n.virt) - e.virt;
            let stored = if e.kind == 0 { len } else { e.phys_size as u64 };
            if ![0, 1, 3].contains(&e.kind)
                || (e.kind != 1 && (e.phys > at || stored > at - e.phys))
                || (e.kind == 3 && len > 64 << 20)
            {
                return fmt(format!("{what}: compression table entry {i} is damaged (type {}, {len:#x} bytes at {:#x})", e.kind, e.virt));
            }
        }
        Ok(Compressed {
            inner,
            head,
            entries,
            data_size: total,
            cache: Mutex::new(vec![]),
            what: what.to_string(),
        })
    }

    fn block(&self, i: usize) -> Result<Arc<Vec<u8>>> {
        if let Some((_, b)) = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(j, _)| *j == i)
        {
            return Ok(b.clone());
        }
        let e = self.entries[i];
        let len = (self.entries.get(i + 1).map_or(self.data_size, |n| n.virt) - e.virt) as usize;
        let raw = self.inner.read(self.head + e.phys, e.phys_size as u64)?;
        let out = lz4_flex::block::decompress(&raw, len).map_err(|err| {
            PackageError::Format(format!("{}: LZ4 block at {:#x}: {err}", self.what, e.virt))
        })?;
        if out.len() != len {
            return fmt(format!(
                "{}: LZ4 block at {:#x} decoded to {:#x} bytes, {len:#x} expected",
                self.what,
                e.virt,
                out.len()
            ));
        }
        let b = Arc::new(out);
        let mut c = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if c.len() >= CACHE {
            c.remove(0);
        }
        c.push((i, b.clone()));
        Ok(b)
    }

    /// Decompressed bytes of the data level.
    fn read_data(&self, offset: u64, size: u64, out: &mut Vec<u8>) -> Result<()> {
        let end = offset + size;
        let mut pos = offset;
        let mut i = self
            .entries
            .partition_point(|e| e.virt <= pos)
            .saturating_sub(1);
        while pos < end {
            let e = self.entries[i];
            let e_end = self.entries.get(i + 1).map_or(self.data_size, |n| n.virt);
            let n = e_end.min(end) - pos;
            let rel = pos - e.virt;
            match e.kind {
                0 => out.extend_from_slice(&self.inner.read(self.head + e.phys + rel, n)?),
                1 => out.resize(out.len() + n as usize, 0),
                _ => out.extend_from_slice(&self.block(i)?[rel as usize..(rel + n) as usize]),
            }
            pos += n;
            i += 1;
        }
        Ok(())
    }
}

impl Storage for Compressed {
    fn size(&self) -> u64 {
        self.head + self.data_size
    }
    fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        if offset.checked_add(size).map_or(true, |e| e > self.size()) {
            return fmt(format!(
                "{}: read past the end of a compressed section",
                self.what
            ));
        }
        let mut out = Vec::with_capacity(size as usize);
        let mut pos = offset;
        if pos < self.head {
            let n = (self.head - pos).min(size);
            out.extend_from_slice(&self.inner.read(pos, n)?);
            pos += n;
        }
        if pos < offset + size {
            self.read_data(pos - self.head, offset + size - pos, &mut out)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `head` bytes of hash levels, then the data level: plain, zero and LZ4 runs followed by their table.
    fn build(head: usize, runs: &[(u8, Vec<u8>)]) -> (Vec<u8>, (u64, u64), Vec<u8>) {
        let mut data = vec![];
        let mut entries = vec![];
        let mut want = vec![];
        for (kind, bytes) in runs {
            let stored = match kind {
                0 => bytes.clone(),
                1 => vec![],
                _ => lz4_flex::block::compress(bytes),
            };
            entries.push((
                want.len() as u64,
                data.len() as u64,
                *kind,
                stored.len() as u32,
            ));
            data.extend_from_slice(&stored);
            if *kind == 1 {
                want.resize(want.len() + bytes.len(), 0);
            } else {
                want.extend_from_slice(bytes);
            }
        }
        let at = data.len() as u64;
        let mut t = vec![0u8; 2 * NODE];
        t[4..8].copy_from_slice(&1u32.to_le_bytes());
        t[8..16].copy_from_slice(&(want.len() as u64).to_le_bytes());
        t[NODE + 4..NODE + 8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        t[NODE + 8..NODE + 16].copy_from_slice(&(want.len() as u64).to_le_bytes());
        for (i, (v, p, k, s)) in entries.iter().enumerate() {
            let e = NODE + 0x10 + i * ENTRY;
            t[e..e + 8].copy_from_slice(&v.to_le_bytes());
            t[e + 8..e + 16].copy_from_slice(&p.to_le_bytes());
            t[e + 16] = *k;
            t[e + 20..e + 24].copy_from_slice(&s.to_le_bytes());
        }
        let size = t.len() as u64;
        data.extend_from_slice(&t);
        let hashes: Vec<u8> = (0..head).map(|i| (i * 7) as u8).collect();
        let mut section = hashes.clone();
        section.extend_from_slice(&data);
        let mut view = hashes;
        view.extend_from_slice(&want);
        (section, (at, size), view)
    }

    #[test]
    fn reads_hash_levels_and_plain_zero_and_lz4_runs() {
        let text: Vec<u8> = (0..70_000u32)
            .map(|i| (i % 251) as u8 ^ (i / 1000) as u8)
            .collect();
        let (section, table, want) = build(
            0x300,
            &[
                (0, b"plain head".to_vec()),
                (3, text),
                (1, vec![0; 5000]),
                (3, b"tail".repeat(100)),
            ],
        );
        let c = Compressed::new(Arc::new(section), 0x300, table, "test").unwrap();
        assert_eq!(c.size(), want.len() as u64);
        assert_eq!(c.read(0, want.len() as u64).unwrap(), want);
        for (at, n) in [
            (0x2f0u64, 0x20u64),
            (0x305, 20),
            (0x309, 70_010),
            (70_300, 3000),
            (want.len() as u64 - 7, 7),
        ] {
            assert_eq!(
                c.read(at, n).unwrap(),
                &want[at as usize..(at + n) as usize]
            );
        }
        assert!(c.read(want.len() as u64 - 1, 2).is_err());
    }

    /// The owner's Super Mario Party Jamboree card image (compressed RomFS). The IVFC hashes cover the stored
    /// (compressed) data level; every NRO read through the LZ4 blocks hashes into the game's NRR list.
    /// `cargo test -p translator --release -- --ignored real_xci`
    #[test]
    #[ignore]
    fn real_xci_compressed_romfs() {
        use sha2::{Digest, Sha256};
        let p = std::path::Path::new(
            r"P:\Games\Nintendo\DUMPS\Super Mario Party Jamboree [0100965017338800]\Super Mario Party Jamboree [0100965017338000][USA][v0][Base].xci",
        );
        if !p.exists() {
            return eprintln!("skip: {}", p.display());
        }
        let keys =
            crate::package::keys::Keys::load(&crate::package::keys::keys_path(None).unwrap())
                .unwrap();
        let pkg =
            crate::package::app::Package::open(p, &keys, &crate::package::UpdateChoice::Embedded)
                .unwrap();
        let si = pkg.program.section(3, 0).unwrap();
        let fs = &pkg.program.sections[si].fs;
        let stored = crate::package::nca::SectionStorage::new(&pkg.program, si);
        let levels = u32le(fs, 0x14) as usize;
        let hash_at = u64le(fs, 0x18 + (levels - 3) * 0x18);
        let (data_at, data_size) = data_level(fs).unwrap();
        let blocks = data_size.div_ceil(0x4000);
        for b in (0..blocks).step_by(997) {
            let mut d = stored
                .read(data_at + b * 0x4000, 0x4000.min(data_size - b * 0x4000))
                .unwrap();
            d.resize(0x4000, 0);
            assert_eq!(
                Sha256::digest(&d)[..],
                stored.read(hash_at + b * 32, 32).unwrap()[..],
                "block {b}"
            );
        }
        let romfs = pkg.romfs().unwrap();
        let nrr = romfs.read_file(".nrr/bq_Release.nrr").unwrap();
        let (at, n) = (u32le(&nrr, 0x340) as usize, u32le(&nrr, 0x344) as usize);
        let listed: std::collections::HashSet<&[u8]> = (0..n)
            .map(|i| &nrr[at + 32 * i..at + 32 * i + 32])
            .collect();
        let nros: Vec<String> = romfs
            .walk()
            .into_iter()
            .map(|x| x.0)
            .filter(|p| p.ends_with(".nro"))
            .collect();
        for f in &nros {
            assert!(
                listed.contains(&Sha256::digest(romfs.read_file(f).unwrap())[..]),
                "{f}"
            );
        }
        eprintln!(
            "{} stored blocks checked, {} NROs match the NRR",
            blocks.div_ceil(997),
            nros.len()
        );
    }

    /// Jamboree's 1.1.0 update over the card image: the patch has its own compression table and relocates onto the
    /// base's stored (compressed) bytes; its RomFS opens and its NROs hash into the NRR.
    /// `cargo test -p translator --release -- --ignored real_update`
    #[test]
    #[ignore]
    fn real_update_over_compressed_base() {
        use sha2::{Digest, Sha256};
        let j = std::path::Path::new(
            r"P:\Games\Nintendo\DUMPS\Super Mario Party Jamboree [0100965017338800]",
        );
        let game = j.join("Super Mario Party Jamboree [0100965017338000][USA][v0][Base].xci");
        let update = j.join(
            r"Updates\Super Mario Party Jamboree [0100965017338800][USA][v65536][Update].nsp",
        );
        if !game.exists() || !update.exists() {
            return eprintln!("skip: {}", j.display());
        }
        let keys =
            crate::package::keys::Keys::load(&crate::package::keys::keys_path(None).unwrap())
                .unwrap();
        let pkg = crate::package::app::Package::open(
            &game,
            &keys,
            &crate::package::UpdateChoice::Package(update),
        )
        .unwrap();
        assert_eq!(pkg.version, 65536);
        let romfs = pkg.romfs().unwrap();
        let nrr = romfs.read_file(".nrr/bq_Release.nrr").unwrap();
        let (at, n) = (u32le(&nrr, 0x340) as usize, u32le(&nrr, 0x344) as usize);
        let listed: std::collections::HashSet<&[u8]> = (0..n)
            .map(|i| &nrr[at + 32 * i..at + 32 * i + 32])
            .collect();
        let nros: Vec<String> = romfs
            .walk()
            .into_iter()
            .map(|x| x.0)
            .filter(|p| p.ends_with(".nro"))
            .collect();
        assert!(!nros.is_empty());
        for f in &nros {
            assert!(
                listed.contains(&Sha256::digest(romfs.read_file(f).unwrap())[..]),
                "{f}"
            );
        }
    }
}

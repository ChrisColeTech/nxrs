//! NCA3 headers, content keys, sections and the NCAs of a package.
//!
//! NCA header: AES-XTS with header_key (Nintendo's big-endian sector tweak, 0x200 sectors). Content key: the
//! ticket's title key unwrapped with `titlekek_XX` for rights-id NCAs, else the header key area
//! (`key_area_key_<application|ocean|system>_XX`). Sections: AES-CTR; counter = the section's 8 counter bytes
//! reversed, then the NCA offset / 16 big-endian. A section of type 4 (AesCtrEx) takes the counter generation of
//! each range from the update's AesCtrEx table (see `bktr`).
use crate::package::container::{self, Member};
use crate::package::crypto::{ctr, ecb_decrypt, xts, MEDIA};
use crate::package::error::{fmt, PackageError, Result};
use crate::package::io::{hex, u32le, u64le, Blob};
use crate::package::keys::Keys;
use crate::package::storage::Storage;
use std::collections::HashMap;
use std::sync::Arc;

pub const HEADER_SIZE: u64 = 0xc00;

/// rights id -> wrapped title key (None: a personalized ticket, bound to one console).
pub type Tickets = HashMap<[u8; 16], Option<[u8; 16]>>;

/// (rights id, wrapped title key or None for a personalized ticket) of a ticket blob.
pub fn ticket_key(data: &[u8]) -> Option<([u8; 16], Option<[u8; 16]>)> {
    if data.len() < 4 {
        return None;
    }
    let body = match u32le(data, 0) {
        0x10000 | 0x10003 => 0x240,
        0x10001 | 0x10004 => 0x140,
        0x10002 | 0x10005 => 0x80,
        _ => return None,
    };
    if data.len() < body + 0x170 {
        return None;
    }
    let rights: [u8; 16] = data[body + 0x160..body + 0x170].try_into().unwrap();
    let common = data[body + 0x141] == 0;
    Some((
        rights,
        common.then(|| data[body + 0x40..body + 0x50].try_into().unwrap()),
    ))
}

/// One filesystem section of an NCA (offsets in the NCA).
#[derive(Debug, Clone)]
pub struct Section {
    pub index: usize,
    pub offset: u64,
    pub size: u64,
    pub partition: u8,
    pub fs_type: u8,
    pub crypto: u8,
    /// Secure value then generation, both big-endian (the FS header's 8 bytes reversed).
    pub upper: [u8; 8],
    pub patch: bool,
    pub sparse: bool,
    pub compressed: bool,
    pub fs: Vec<u8>,
}

impl Section {
    pub fn kind(&self) -> &'static str {
        match (self.fs_type, self.partition) {
            (2, 1) => "pfs0",
            (3, 0) if self.patch => "romfs(bktr)",
            (3, 0) => "romfs",
            _ => "other",
        }
    }
}

pub struct Nca {
    pub blob: Arc<Blob>,
    pub name: String,
    /// Offset and size of the NCA inside its package file.
    pub offset: u64,
    #[allow(dead_code)]
    pub size: u64,
    pub content_type: u8,
    pub title_id: u64,
    pub key_generation: u8,
    pub rights: [u8; 16],
    pub sections: Vec<Section>,
    pub key: Option<[u8; 16]>,
    /// `<key name>:<wrapped content key hex>` as the package holds it (for the runtime marker), None when plain.
    pub wrapped: Option<String>,
}

impl std::fmt::Debug for Nca {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Nca({}, type {}, title {:016x})",
            self.name, self.content_type, self.title_id
        )
    }
}

pub fn content_type_name(t: u8) -> &'static str {
    match t {
        0 => "Program",
        1 => "Meta",
        2 => "Control",
        3 => "Manual",
        4 => "Data",
        5 => "PublicData",
        _ => "?",
    }
}

/// Decrypt an NCA header (0xc00 bytes).
pub fn decrypt_header(blob: &Blob, offset: u64, keys: &Keys) -> Result<Vec<u8>> {
    let mut header = blob.read_at(offset, HEADER_SIZE)?;
    xts(&keys.header_key()?, &mut header, 0, MEDIA, false);
    Ok(header)
}

impl Nca {
    /// nca.py `Nca(path, member, keys, tickets)`: an Err carries Turbo's reason.
    pub fn open(blob: &Arc<Blob>, member: &Member, keys: &Keys, tickets: &Tickets) -> Result<Nca> {
        let path = blob.path.display();
        let name = &member.name;
        if member.size < HEADER_SIZE {
            return fmt(format!("{path}: {name} is too small for an NCA"));
        }
        let header = decrypt_header(blob, member.offset, keys)?;
        if &header[0x200..0x204] != b"NCA3" {
            return fmt(format!(
                "{path}: the installed keys do not open {name} (or it is not an NCA3)"
            ));
        }
        let content_type = header[0x205];
        let title_id = u64le(&header, 0x210);
        let key_generation = header[0x206].max(header[0x220]).saturating_sub(1);
        let rights: [u8; 16] = header[0x230..0x240].try_into().unwrap();
        let mut sections = Vec::new();
        for index in 0..4 {
            let start = u32le(&header, 0x240 + index * 0x10) as u64;
            let end = u32le(&header, 0x244 + index * 0x10) as u64;
            if start == 0 && end == 0 {
                continue;
            }
            if end < start || end * MEDIA as u64 > member.size {
                return fmt(format!("{path}: an NCA section of {name} is out of range"));
            }
            let fs = header[0x400 + index * 0x200..0x600 + index * 0x200].to_vec();
            let mut upper: [u8; 8] = fs[0x140..0x148].try_into().unwrap();
            upper.reverse();
            let crypto = fs[4];
            sections.push(Section {
                index,
                offset: start * MEDIA as u64,
                size: (end - start) * MEDIA as u64,
                partition: fs[2],
                fs_type: fs[3],
                crypto,
                upper,
                patch: &fs[0x110..0x114] == b"BKTR" || crypto == 4,
                sparse: fs[0x148..0x178].iter().any(|&b| b != 0),
                compressed: fs[0x178..0x1a0].iter().any(|&b| b != 0),
                fs,
            });
        }
        let mut nca = Nca {
            blob: blob.clone(),
            name: name.clone(),
            offset: member.offset,
            size: member.size,
            content_type,
            title_id,
            key_generation,
            rights,
            sections,
            key: None,
            wrapped: None,
        };
        if nca.sections.iter().all(|s| s.crypto == 1) {
            return Ok(nca);
        }
        let (kek_name, wrapped) = if rights.iter().any(|&b| b != 0) {
            match tickets.get(&rights) {
                None => {
                    return fmt(format!("{path}: {name} needs the ticket {}.tik and the package has none", hex(&rights)))
                }
                Some(None) => {
                    return fmt(format!(
                        "{path}: the ticket {} is bound to one console (a personalized ticket); dump the game with a \
                         common ticket",
                        hex(&rights)
                    ))
                }
                Some(Some(w)) => (format!("titlekek_{key_generation:02x}"), *w),
            }
        } else {
            let area = match header[0x207] {
                0 => "application",
                1 => "ocean",
                2 => "system",
                _ => return fmt(format!("{path}: {name} uses an unknown key area key")),
            };
            (
                format!("key_area_key_{area}_{key_generation:02x}"),
                header[0x320..0x330].try_into().unwrap(),
            )
        };
        let kek = keys.get16(&kek_name)?;
        nca.key = Some(ecb_decrypt(&kek, &wrapped)[..].try_into().unwrap());
        nca.wrapped = Some(format!("{kek_name}:{}", hex(&wrapped)));
        Ok(nca)
    }

    pub fn is_patch(&self) -> bool {
        self.sections.iter().any(|s| s.patch)
    }

    pub fn section(&self, fs_type: u8, partition: u8) -> Option<usize> {
        self.sections
            .iter()
            .position(|s| s.fs_type == fs_type && s.partition == partition)
    }

    /// The raw (still encrypted) bytes of a section range.
    pub fn raw(&self, s: &Section, offset: u64, size: u64) -> Result<Vec<u8>> {
        if offset.checked_add(size).map_or(true, |e| e > s.size) {
            return fmt(format!(
                "{}: read past the end of an NCA section",
                self.blob.path.display()
            ));
        }
        self.blob.read_at(self.offset + s.offset + offset, size)
    }

    /// Read and decrypt a section range; `generation` overrides the counter's generation (AesCtrEx ranges).
    pub fn read(
        &self,
        s: &Section,
        offset: u64,
        size: u64,
        generation: Option<u32>,
    ) -> Result<Vec<u8>> {
        let mut data = self.raw(s, offset, size)?;
        if s.crypto != 1 {
            let key = self.key.ok_or_else(|| {
                PackageError::Format(format!(
                    "{}: {} has no content key",
                    self.blob.path.display(),
                    self.name
                ))
            })?;
            let mut upper = s.upper;
            if let Some(g) = generation {
                upper[4..].copy_from_slice(&g.to_be_bytes());
            }
            ctr(&key, &upper, s.offset + offset, &mut data);
        }
        Ok(data)
    }

    /// nca.py `Section.ivfc_data`: (offset, size) of the RomFS image, the last IVFC level, in the section.
    pub fn ivfc_data(&self, s: &Section) -> Result<(u64, u64)> {
        if &s.fs[8..12] != b"IVFC" {
            return fmt(format!(
                "{}: the RomFS section has no IVFC header",
                self.blob.path.display()
            ));
        }
        let at = 0x18 + 5 * 0x18;
        Ok((u64le(&s.fs, at), u64le(&s.fs, at + 8)))
    }

    /// nca.py `Section.layer`: how the runtime reaches the section: file offset, size, NCA offset, crypto, counter
    /// and the content key still wrapped (`-` for plaintext).
    pub fn layer(&self, s: &Section) -> String {
        let key = if s.crypto != 1 {
            self.wrapped.clone().unwrap_or_else(|| "None".into())
        } else {
            "-".into()
        };
        format!(
            "{:x} {:x} {:x} {} {} {}",
            self.offset + s.offset,
            s.size,
            s.offset,
            s.crypto,
            hex(&s.upper),
            key
        )
    }

    pub fn key_status(&self) -> &str {
        if self.sections.iter().all(|s| s.crypto == 1) {
            "plain"
        } else {
            "key ok"
        }
    }
}

/// One AES-CTR (or plaintext) NCA section as a byte storage: what the runtime marker's `base` / `patch` rows
/// describe (file offset, size, offset in the NCA, crypto, counter, content key).
#[derive(Clone)]
pub struct SectionStorage {
    pub blob: Arc<Blob>,
    pub file_offset: u64,
    pub size: u64,
    pub nca_offset: u64,
    pub crypto: u8,
    pub upper: [u8; 8],
    pub key: Option<[u8; 16]>,
    /// The section's FS header (BKTR table locations), empty when built from a marker.
    pub fs: Vec<u8>,
}

impl SectionStorage {
    pub fn new(nca: &Arc<Nca>, index: usize) -> SectionStorage {
        let s = &nca.sections[index];
        SectionStorage {
            blob: nca.blob.clone(),
            file_offset: nca.offset + s.offset,
            size: s.size,
            nca_offset: s.offset,
            crypto: s.crypto,
            upper: s.upper,
            key: nca.key,
            fs: s.fs.clone(),
        }
    }

    /// Read and decrypt; `generation` overrides the counter's generation (AesCtrEx ranges of a patch).
    pub fn read_gen(&self, offset: u64, size: u64, generation: Option<u32>) -> Result<Vec<u8>> {
        if offset.checked_add(size).map_or(true, |e| e > self.size) {
            return fmt(format!(
                "{}: read past the end of an NCA section",
                self.blob.path.display()
            ));
        }
        let mut data = self.blob.read_at(self.file_offset + offset, size)?;
        if self.crypto != 1 {
            let key = self.key.ok_or_else(|| {
                PackageError::Format(format!("{}: no content key", self.blob.path.display()))
            })?;
            let mut upper = self.upper;
            if let Some(g) = generation {
                upper[4..].copy_from_slice(&g.to_be_bytes());
            }
            ctr(&key, &upper, self.nca_offset + offset, &mut data);
        }
        Ok(data)
    }
}

impl Storage for SectionStorage {
    fn size(&self) -> u64 {
        self.size
    }
    fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        self.read_gen(offset, size, None)
    }
}

/// nca.py `open_ncas`: the NCAs of a package that the keys open, and how many NCA headers they did not open.
/// Compressed members (.ncz) are refused: they cannot be read in place.
pub fn open_ncas(blob: &Arc<Blob>, keys: &Keys) -> Result<(Vec<Arc<Nca>>, usize)> {
    let path = blob.path.display();
    let members = container::members(blob)?;
    let bare = container::extension(&blob.path) == ".nca";
    let mut blobs = vec![];
    for m in members
        .iter()
        .filter(|m| m.name.to_ascii_lowercase().ends_with(".tik") && m.size <= 0x10000)
    {
        blobs.push(blob.read_at(m.offset, m.size)?);
    }
    if bare {
        if let Some(folder) = blob.path.parent() {
            let mut names: Vec<_> = std::fs::read_dir(folder)
                .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect())
                .unwrap_or_default();
            names.sort();
            for p in names {
                let is_tik = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("tik"));
                if is_tik
                    && std::fs::metadata(&p)
                        .map(|m| m.len() <= 0x10000)
                        .unwrap_or(false)
                {
                    if let Ok(b) = std::fs::read(&p) {
                        blobs.push(b);
                    }
                }
            }
        }
    }
    let mut tickets = Tickets::new();
    for b in &blobs {
        if let Some((rights, wrapped)) = ticket_key(b) {
            if tickets.get(&rights).copied().flatten().is_none() {
                tickets.insert(rights, wrapped);
            }
        }
    }
    let mut ncas = vec![];
    let mut locked = 0;
    for m in &members {
        let name = m.name.to_ascii_lowercase();
        if name.ends_with(".ncz") && !name.ends_with(".cnmt.ncz") {
            return fmt(format!(
                "{path}: this is a compressed package (NSZ / XCZ). Its data cannot be read in place: decompress it \
                 to an NSP or XCI first (nsz -D)"
            ));
        }
        if !name.ends_with(".nca") {
            continue;
        }
        match Nca::open(blob, m, keys, &tickets) {
            Ok(n) => ncas.push(Arc::new(n)),
            Err(e) => {
                if name.ends_with(".cnmt.nca") {
                    continue;
                }
                let header = if m.size >= HEADER_SIZE {
                    decrypt_header(blob, m.offset, keys)?
                } else {
                    vec![]
                };
                if header.get(0x200..0x204) != Some(b"NCA3") {
                    locked += 1;
                    continue;
                }
                if header[0x205] == 0 {
                    return Err(e); // a Program NCA that the keys or tickets don't open: say why
                }
            }
        }
    }
    Ok((ncas, locked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_common_and_personalized() {
        let mut t = vec![0u8; 0x240 + 0x180];
        t[..4].copy_from_slice(&0x10004u32.to_le_bytes()); // RSA-2048 SHA256: body at 0x140
        let body = 0x140;
        t[body + 0x40..body + 0x50].copy_from_slice(&[7u8; 16]);
        t[body + 0x160..body + 0x170].copy_from_slice(&[9u8; 16]);
        let (rights, key) = ticket_key(&t).unwrap();
        assert_eq!(rights, [9u8; 16]);
        assert_eq!(key, Some([7u8; 16]));
        t[body + 0x141] = 1;
        assert_eq!(ticket_key(&t).unwrap().1, None);
        assert!(ticket_key(&[1, 2, 3, 4]).is_none());
    }
}

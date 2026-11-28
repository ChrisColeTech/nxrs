//! the nxrs-side open path for a 3DS title: which kind of dump it is, and the title behind it
//! the W0 form: detection and the title only. `write_data` (staging, the RomFs adapter,
//! `control.smdh`, the `romfs.ds` marker) is 3DS-W1 — docs/waves/3DS-W0.md, "W1 owns"

use crate::rom::{self, Blob, Error, Title};
use std::path::{Path, PathBuf};

/// the container a title came in as
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsKind {
    /// a cart image: NCSD at 0x100, the exe as its first partition
    Cart,
    /// a CIA: header, certs, ticket, TMD, the exe as content 0
    Cia,
    /// the exe partition alone: NCCH magic at 0x100 (its signature occupies the first 0x100 bytes)
    Cxi,
    /// the same NCCH layout, arrived as a bare content file; only the name tells it from a .cxi
    App,
}

/// a 3DS title opened for recompiling, and the container it came from
pub struct Game3ds {
    pub kind: DsKind,
    pub title: Title,
    /// the dump it came from, absolute
    pub path: PathBuf,
}

/// opens a dump by its container kind, then reads the title's exe partition out of it
/// a `.app` is recorded as App when its magic says NCCH; every kind the magic detects reads
/// through the donor's detection, so the refusals stay its exact lines
pub fn open(path: impl AsRef<Path>) -> Result<Game3ds, Error> {
    let path = path.as_ref();
    let blob = Blob::open(path)?;
    let mut kind = kind_of(&blob.read_at(0, 0x200)?)?;
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app")) && kind == DsKind::Cxi {
        kind = DsKind::App;
    }
    let title = Title::open(blob)?;
    Ok(Game3ds {
        kind,
        title,
        path: std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    })
}

/// the container kind by magic: NCSD at 0x100, NCCH at 0x100, or a CIA header size at 0
/// a .cxi and a bare .app are the same bytes; the caller names the App kind from the extension
pub fn kind_of(start: &[u8]) -> Result<DsKind, Error> {
    let magic = |offset: usize, word: &[u8; 4]| start.get(offset..offset + 4).is_some_and(|got| got == word);
    if magic(0x100, b"NCSD") {
        Ok(DsKind::Cart)
    } else if magic(0x100, b"NCCH") {
        Ok(DsKind::Cxi)
    } else if start.first_chunk::<4>().is_some_and(|got| u32::from_le_bytes(*got) == rom::CIA_HEADER_SIZE) {
        Ok(DsKind::Cia)
    } else {
        rom::error("not a 3DS cartridge image, .cia or executable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::tests::rom;

    fn words(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    /// the .cxi fixture reads as a Cxi, a cart wrap of it as a Cart, and the same bytes named .app as an App
    #[test]
    fn kinds_by_magic_and_name() {
        let path = rom("kinds", &[1, 2, 3, 4], &[]);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(kind_of(&bytes).unwrap(), DsKind::Cxi);

        // a cart: 0x100 of lead-in, NCSD at 0x100, the fixture as partition 0 at 0x600
        let mut cart = vec![0; 0x600];
        cart[0x100..0x104].copy_from_slice(b"NCSD");
        cart[0x120..0x124].copy_from_slice(&((0x600u32 / 0x200)).to_le_bytes());
        cart.extend_from_slice(&bytes);
        assert_eq!(kind_of(&cart).unwrap(), DsKind::Cart);
        let opened = open(&path).unwrap();
        assert_eq!(opened.kind, DsKind::Cxi);
        assert_eq!(opened.title.program_id(), 0x0004_0000_0FF3_DE00);
        assert_eq!(opened.title.code().unwrap(), bytes[0x800..]);
        drop(opened);

        // the same NCCH named .app
        let app_path = path.with_extension("app");
        std::fs::rename(&path, &app_path).unwrap();
        let opened = open(&app_path).unwrap();
        assert_eq!(opened.kind, DsKind::App);
        assert_eq!(opened.title.code().unwrap(), bytes[0x800..]);
        drop(opened);
        std::fs::remove_file(app_path).unwrap();

        // nothing recognizable is refused with the container line
        assert_eq!(kind_of(&[0u8; 0x200]).unwrap_err().to_string(), "not a 3DS cartridge image, .cia or executable");
    }

    /// a .cia whose content 0 is plain carries the exe, an encrypted one is refused the donor's way
    #[test]
    fn a_cia_opens_through_its_content_record() {
        let code = words(&[0xE1A0_0000, 0xE12F_FF1E]);
        let bytes = cia(&code, false);
        let path = std::env::temp_dir().join(format!("translator3ds-kinds-cia-{}.cia", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let opened = open(&path).unwrap();
        assert_eq!(opened.kind, DsKind::Cia);
        assert_eq!(opened.title.code().unwrap(), code);
        drop(opened);

        let encrypted = cia(&code, true);
        std::fs::write(&path, &encrypted).unwrap();
        let Err(message) = open(&path) else {
            panic!("an encrypted .cia opened");
        };
        assert_eq!(message.to_string(), "the .cia is encrypted, it has to be decrypted first");
        std::fs::remove_file(path).unwrap();
    }

    /// the tiniest .cia: header (its own size, no certs or ticket, a TMD), bitmap, TMD, content 0
    /// the content is the tiniest .cxi (content 0 of a game CIA is the exe's NCCH partition),
    /// code is its .code bytes, encrypted sets the record's type bit 0
    fn cia(code: &[u8], encrypted: bool) -> Vec<u8> {
        let content = crate::rom::tests::rom_bytes(code, &[]);
        let mut out = vec![0u8; 0x2020];
        out[..4].copy_from_slice(&0x2020u32.to_le_bytes());
        // content bitmap: content 0 present; cert and ticket sizes stay 0
        out[0x20] = 0x80;
        // the TMD starts on a 64 byte boundary after the 0x2020 header, and the content after it
        let tmd_at = 0x2040usize;
        let sig = 0x140;
        let record = tmd_at + sig + 0xC4 + 64 * 0x24;
        let tmd_size = record + 0x30 - tmd_at;
        let content_at = (tmd_at + tmd_size).next_multiple_of(0x40);
        out.resize(content_at + content.len(), 0);
        // the header names the TMD's size, and the TMD its signature type (0x00010004, RSA-2048 padded),
        // title id and one content record
        out[0x10..0x14].copy_from_slice(&(tmd_size as u32).to_le_bytes());
        out[tmd_at..tmd_at + 4].copy_from_slice(&0x0001_0004u32.to_be_bytes());
        let title_id_at = tmd_at + sig + 0x4C;
        out[title_id_at..title_id_at + 8].copy_from_slice(&0x0004_0000_0FF3_DE00u64.to_be_bytes());
        let count_at = tmd_at + sig + 0x9E;
        out[count_at..count_at + 2].copy_from_slice(&1u16.to_be_bytes());
        out[record + 4..record + 6].copy_from_slice(&0u16.to_be_bytes());
        out[record + 8..record + 16].copy_from_slice(&(content.len() as u64).to_be_bytes());
        let kind = if encrypted { 1u16 } else { 0 };
        out[record + 6..record + 8].copy_from_slice(&kind.to_be_bytes());
        out[content_at..].copy_from_slice(&content);
        out
    }
}

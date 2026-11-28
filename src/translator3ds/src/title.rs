//! the nxrs-side open path for a 3DS title: which kind of dump it is, the title behind it, and the
//! data folder a build stages (`write_data`, the Switch flow's layout: programs, the RomFS through
//! a `translator::package::romfs`-shaped adapter, `control.smdh`, the `romfs.ds` marker record)

use crate::rom::{self, Blob, Error, Title};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use translator::package::PackageError;
use translator::package::extract;
use translator::package::marker;
use translator::package::romfs::{RomFs, StoredFiles};

/// the marker `romfs.source` names for a 3DS build (the Switch markers name NSP/NUP/folder)
pub const MARKER: &str = "romfs.ds";

/// the ExeFS `.icon` (an SMDH) staged as the data folder's control file
pub const CONTROL: &str = "control.smdh";

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
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        && kind == DsKind::Cxi
    {
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
    let magic = |offset: usize, word: &[u8; 4]| {
        start.get(offset..offset + 4).is_some_and(|got| got == word)
    };
    if magic(0x100, b"NCSD") {
        Ok(DsKind::Cart)
    } else if magic(0x100, b"NCCH") {
        Ok(DsKind::Cxi)
    } else if start
        .first_chunk::<4>()
        .is_some_and(|got| u32::from_le_bytes(*got) == rom::CIA_HEADER_SIZE)
    {
        Ok(DsKind::Cia)
    } else {
        rom::error("not a 3DS cartridge image, .cia or executable")
    }
}

/// the container in the words a build log names it by
impl std::fmt::Display for DsKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DsKind::Cart => "3DS cart image",
            DsKind::Cia => ".cia",
            DsKind::Cxi => ".cxi",
            DsKind::App => ".app",
        })
    }
}

/// the dump's RomFS as the Switch-side machinery reads it: the 3DS tables walked into
/// (path, size, id) triples, each file's bytes read from the dump on demand
struct DumpFiles {
    title: Title,
    /// the RomFS file of each id, in walk order
    entries: Vec<rom::FileEntry>,
}

impl StoredFiles for DumpFiles {
    fn read(&self, id: u32, offset: u64, len: u64) -> Result<Vec<u8>, PackageError> {
        self.entries
            .get(id as usize)
            .and_then(|file| self.title.read_romfs(file, offset, len as usize))
            .ok_or_else(|| {
                PackageError::Format(format!("the RomFS file #{id} ends before its size"))
            })
    }
}

/// path of name under the folder at path; nameless folders (the root and the ones some titles
/// stuff everything under) add nothing, as the module paths spell it too
fn join(path: &str, name: &str) -> String {
    if path.is_empty() || name.is_empty() {
        format!("{path}{name}")
    } else {
        format!("{path}/{name}")
    }
}

fn walk(
    romfs: &rom::RomFs,
    dir: &rom::DirEntry,
    path: &str,
    files: &mut Vec<(String, u64, u32)>,
    entries: &mut Vec<rom::FileEntry>,
) {
    for (_, file) in romfs.files(dir) {
        files.push((join(path, &file.name), file.data_size, entries.len() as u32));
        entries.push(file);
    }
    for (_, subdir) in romfs.subdirs(dir) {
        walk(romfs, &subdir, &join(path, &subdir.name), files, entries);
    }
}

/// `<size> <modification time, ns since 1970>` of a file, `- -` when it cannot be read
/// (the Switch record's stamp rows, so the words stay one shape)
fn file_stamp(p: &Path) -> String {
    let Ok(m) = std::fs::metadata(p) else {
        return "- -".into();
    };
    let ns = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{} {ns}", m.len())
}

impl Game3ds {
    /// Stage the data folder a build hands the runtime, the Switch flow's layout:
    /// `exefs/` (the exe's code and its exheader), `romfs/` (extracted through the adapter),
    /// `control.smdh` (the ExeFS `.icon`) and the `romfs.source` record. Says what was applied,
    /// in the words the launcher reads; skips the RomFS while the record says the copy is current.
    pub fn write_data(&self, datadir: &Path, say: &dyn Fn(&str)) -> Result<(), Error> {
        self.write_data_with(datadir, say, &|_, _| {})
    }

    /// [`Game3ds::write_data`], with `progress(bytes, total)` called while the RomFS is written out.
    pub fn write_data_with(
        &self,
        datadir: &Path,
        say: &dyn Fn(&str),
        progress: &dyn Fn(u64, u64),
    ) -> Result<(), Error> {
        let write = |p: &Path, bytes: &[u8]| -> Result<(), Error> {
            std::fs::write(p, bytes).map_err(|source| io_err(p, source))
        };
        say(&format!("Package: {} {}", self.kind, self.path.display()));
        // the exe's program: the code segments back to back, the exheader beside it saying where
        // each sits (the CROs are RomFS files and arrive with the extraction below)
        let ex = datadir.join("exefs");
        std::fs::create_dir_all(&ex).map_err(|source| io_err(&ex, source))?;
        let code_path = ex.join("code");
        write(&code_path, &self.title.code()?)?;
        let exheader_path = ex.join("exheader.bin");
        write(&exheader_path, &self.title.exheader_raw)?;
        self.write_romfs(datadir, say, progress)?;
        if let Some(icon) = self.title.icon() {
            let p = datadir.join(CONTROL);
            write(&p, &icon)?;
        }
        Ok(())
    }

    /// The dump's RomFS into the data folder: `romfs/` (or, for names a Windows folder cannot hold,
    /// the extraction refuses: a 3DS image has no `romfs.bin` shape the runtime reads), recorded in
    /// `romfs.source`; skipped while the record says the copy is current. The in-place markers of
    /// older builds are removed.
    fn write_romfs(
        &self,
        datadir: &Path,
        say: &dyn Fn(&str),
        progress: &dyn Fn(u64, u64),
    ) -> Result<(), Error> {
        for old in [marker::NSP, marker::NUP, marker::DIR] {
            let _ = std::fs::remove_file(datadir.join(old));
        }
        let (folder, record) =
            (datadir.join(marker::FOLDER), datadir.join(marker::SOURCE));
        let Some(romfs) = &self.title.romfs else {
            // no RomFS: the runtime finds an empty romfs/ folder
            let _ = std::fs::remove_file(&record);
            std::fs::create_dir_all(&folder)
                .map_err(|source| io_err(&folder, source))?;
            say(&format!("RomFS: {} has none", self.path.display()));
            return Ok(());
        };
        let mut files: Vec<(String, u64, u32)> = Vec::new();
        let mut entries: Vec<rom::FileEntry> = Vec::new();
        if let Ok(root) = romfs.root() {
            walk(romfs, &root, "", &mut files, &mut entries);
        }
        let stored = RomFs::from_stored(
            &files,
            Arc::new(DumpFiles {
                title: Title::open(Blob::open(&self.path)?)?,
                entries,
            }),
            format!("3ds {}", self.path.display()),
        )
        .map_err(Error::from)?;
        let bytes: u64 = files.iter().map(|f| f.1).sum();
        let text = format!(
            "{}\nmarker {MARKER}\nextracted {} {} {bytes}\nstamp {} {}\n",
            self.path.display(),
            marker::FOLDER,
            files.len(),
            file_stamp(&self.path),
            self.path.display(),
        );
        if folder.is_dir() && std::fs::read_to_string(&record).is_ok_and(|t| t == text) {
            say("RomFS: romfs/ is current (unchanged since the last build)");
            return Ok(());
        }
        // the record goes first: a copy cut short is never taken as current
        let _ = std::fs::remove_file(&record);
        let t = std::time::Instant::now();
        say(&format!(
            "RomFS: extracting {} into romfs/",
            self.path.display()
        ));
        let (written, n) =
            extract::tree(&stored, &folder, extract::default_threads(), progress)?;
        let _ = std::fs::remove_file(datadir.join(marker::IMAGE));
        let mb = |b: u64| b as f64 / 1048576.0;
        say(&format!(
            "RomFS: extracted into romfs/ ({written} files, {:.1} MB, {:.1} s)",
            mb(n),
            t.elapsed().as_secs_f64()
        ));
        std::fs::write(&record, text).map_err(|source| io_err(&record, source))?;
        Ok(())
    }
}

fn io_err(p: &Path, source: std::io::Error) -> Error {
    Error::from(format!("{}: {source}", p.display()))
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
        cart[0x120..0x124].copy_from_slice(&(0x600u32 / 0x200).to_le_bytes());
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
        assert_eq!(
            kind_of(&[0u8; 0x200]).unwrap_err().to_string(),
            "not a 3DS cartridge image, .cia or executable"
        );
    }

    /// a .cia whose content 0 is plain carries the exe, an encrypted one is refused the donor's way
    #[test]
    fn a_cia_opens_through_its_content_record() {
        let code = words(&[0xE1A0_0000, 0xE12F_FF1E]);
        let bytes = cia(&code, false);
        let path = std::env::temp_dir().join(format!(
            "translator3ds-kinds-cia-{}.cia",
            std::process::id()
        ));
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
        assert_eq!(
            message.to_string(),
            "the .cia is encrypted, it has to be decrypted first"
        );
        std::fs::remove_file(path).unwrap();
    }

    /// the tiniest .cia: header (its own size, no certs or ticket, a TMD), bitmap, TMD, content 0
    /// the content is the tiniest .cxi (content 0 of a game CIA is the exe's NCCH partition),
    /// code is its .code bytes, encrypted sets the record's type bit 0
    fn cia(code: &[u8], encrypted: bool) -> Vec<u8> {
        cia_content(&crate::rom::tests::rom_bytes(code, &[]), encrypted)
    }

    /// the same .cia around whole content bytes (the staging fixtures pass a .cxi that carries
    /// its own `.icon`)
    fn cia_content(content: &[u8], encrypted: bool) -> Vec<u8> {
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

    /// write_data stages the Switch flow's layout: the exe's code + exheader under exefs/, the
    /// RomFS extracted through the adapter, the `.icon` as control.smdh, and the `romfs.ds`
    /// record; a second run over the same dump finds the copy current
    #[test]
    fn write_data_stages_the_data_folder() {
        use crate::rom::tests::{rom_bytes_icon, smdh};

        let icon = smdh("Staged", "Staged Long", "nxrs", 0x001F);
        let path = std::env::temp_dir().join(format!(
            "translator3ds-stage-{}.cxi",
            std::process::id()
        ));
        std::fs::write(
            &path,
            rom_bytes_icon(
                &words(&[0xE3A0_0000, 0xE12F_FF1E]),
                &[("static.crs", b"crs"), ("cro/Battle.cro", b"battle")],
                Some(&icon),
            ),
        )
        .unwrap();
        let game = open(&path).unwrap();
        let data = std::env::temp_dir().join(format!("translator3ds-stage-{}", std::process::id()));
        std::fs::remove_dir_all(&data).ok();
        std::fs::create_dir_all(&data).unwrap();
        let said = std::sync::Mutex::new(Vec::new());
        game.write_data_with(&data, &|m| said.lock().unwrap().push(m.to_owned()), &|_, _| {})
            .unwrap();
        let read = |rel: &str| std::fs::read(data.join(rel)).unwrap();
        let title = &game.title;
        assert_eq!(read("exefs/code"), title.code().unwrap());
        assert_eq!(read("exefs/exheader.bin"), title.exheader_raw);
        assert_eq!(read("romfs/static.crs"), b"crs");
        assert_eq!(read("romfs/cro/Battle.cro"), b"battle");
        assert_eq!(read(CONTROL), icon);
        let record = std::fs::read_to_string(data.join("romfs.source")).unwrap();
        let rows: Vec<&str> = record.lines().collect();
        assert_eq!(rows[0], path.display().to_string());
        assert_eq!(rows[1], "marker romfs.ds");
        assert!(rows[2].starts_with("extracted romfs 2 "), "{}", rows[2]);
        assert!(rows[3].starts_with("stamp "), "{}", rows[3]);
        // the extraction says itself, then the copy is current
        let said = said.into_inner().unwrap();
        assert!(said.iter().any(|m| m.starts_with("RomFS: extracting")), "{said:?}");
        let said = std::sync::Mutex::new(Vec::new());
        game.write_data_with(&data, &|m| said.lock().unwrap().push(m.to_owned()), &|_, _| {})
            .unwrap();
        assert!(said
            .into_inner()
            .unwrap()
            .iter()
            .any(|m| m.contains("is current")),);
        drop(game);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&data).unwrap();
    }

    /// the same staging through a .cia container: open walks the TMD to content 0, write_data
    /// stages from there
    #[test]
    fn a_cia_stages_like_a_cart() {
        use crate::rom::tests::{rom_bytes_icon, smdh};

        let icon = smdh("Cia", "Cia Long", "nxrs", 0x001F);
        let content = rom_bytes_icon(
            &words(&[0xE3A0_0000, 0xE12F_FF1E]),
            &[("static.crs", b"crs")],
            Some(&icon),
        );
        let bytes = cia_content(&content, false);
        let path = std::env::temp_dir().join(format!(
            "translator3ds-stage-cia-{}.cia",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        let game = open(&path).unwrap();
        assert_eq!(game.kind, DsKind::Cia);
        let data = std::env::temp_dir().join(format!(
            "translator3ds-stage-cia-data-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&data).ok();
        std::fs::create_dir_all(&data).unwrap();
        game.write_data(&data, &|_| {}).unwrap();
        assert_eq!(
            std::fs::read(data.join("exefs/code")).unwrap(),
            game.title.code().unwrap()
        );
        assert_eq!(std::fs::read(data.join(CONTROL)).unwrap(), icon);
        assert_eq!(std::fs::read(data.join("romfs/static.crs")).unwrap(), b"crs");
        let record = std::fs::read_to_string(data.join("romfs.source")).unwrap();
        assert!(record.contains("marker romfs.ds"), "{record}");
        drop(game);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&data).unwrap();
    }

    /// a title without a RomFS stages an empty romfs/ folder and no record; without an `.icon`
    /// it stages no control file
    #[test]
    fn a_title_without_romfs_or_icon_stages_without_them() {
        use crate::rom::tests::rom_bytes;

        let path = std::env::temp_dir().join(format!(
            "translator3ds-stage-bare-{}.cxi",
            std::process::id()
        ));
        std::fs::write(&path, rom_bytes(&words(&[0xE12F_FF1E]), &[])).unwrap();
        let game = open(&path).unwrap();
        let data = std::env::temp_dir().join(format!("translator3ds-stage-bare-{}", std::process::id()));
        std::fs::remove_dir_all(&data).ok();
        std::fs::create_dir_all(&data).unwrap();
        game.write_data(&data, &|_| {}).unwrap();
        assert!(data.join("romfs").is_dir());
        assert!(!data.join("romfs.source").exists());
        assert!(!data.join(CONTROL).exists());
        assert!(data.join("exefs/code").is_file());
        drop(game);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&data).unwrap();
    }
}

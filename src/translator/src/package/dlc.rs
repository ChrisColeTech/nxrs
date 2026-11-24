//! Add-on content (DLC) for a built game: install / sync / remove / list, and each title's RomFS read in place.
//! The manifest format and the messages are Turbo's (its docs/DLC.md).
//!
//! The game's data folder gets:
//! ```text
//! dlc.manifest                  one line a title: <title id> <version> <data|stub> <package>
//! dlc/<title id>/romfs.nsp      a marker: the title's RomFS read in place from its package (SWRNSP1 plaintext,
//!                               SWRNSP2 AES-CTR with the content key still wrapped), or
//! dlc/<title id>/romfs.bin      the RomFS image unpacked once from an NSZ / XCZ (its data is zstd-compressed)
//! dlc/<title id>/install.json   what was installed from where (to skip the work next time)
//! ```
//! One difference from Turbo, on purpose: an AES-CTR title is not decrypted into `romfs.bin` with hactool; it is
//! read in place through a version 2 marker, which Turbo's runtime already reads for games (`nsp_open_marker`).
use crate::package::app::pfs_files;
use crate::package::cnmt::{self, base_of, is_addon_of};
use crate::package::container::{self, Member};
use crate::package::crypto::ecb_decrypt;
use crate::package::error::{fmt, PackageError, Result};
use crate::package::io::{abspath, hex, u32le, u64le, Blob};
use crate::package::keys::{keys_path, Keys};
use crate::package::marker;
use crate::package::nca::{decrypt_header, ticket_key, Nca, SectionStorage, Tickets};
use crate::package::romfs::RomFs;
use crate::package::storage::{Slice, Storage};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One add-on content title of a package: its id, version and the member that holds its data.
#[derive(Debug, Clone)]
pub struct Title {
    pub path: PathBuf,
    pub title_id: u64,
    pub version: u32,
    pub member: Member,
    pub files: Vec<Member>,
}

/// A manifest line: (title id, version, "data" | "stub", package path).
pub type Entry = (u64, u32, String, String);

/// (add-on content titles, the types of the other titles found) of a package.
pub fn scan(path: &Path, keys: &Keys) -> Result<(Vec<Title>, Vec<u8>)> {
    let path = abspath(path);
    let p = path.display();
    if !path.is_file() {
        return fmt(format!("{p}: the package is missing"));
    }
    let blob = Arc::new(Blob::open(&path)?);
    let files = container::members(&blob)?;
    if container::extension(&path) == ".nca" {
        let header = decrypt_header(&blob, 0, keys)
            .ok()
            .filter(|h| matches!(&h[0x200..0x204], b"NCA3" | b"NCA2"));
        let Some(header) = header else {
            return fmt(format!("{p}: the installed keys do not open this NCA"));
        };
        let title_id = u64le(&header, 0x210);
        if !matches!(header[0x205], 4 | 5) || title_id & 0x1000 == 0 {
            return fmt(format!(
                "{p}: this NCA is not add-on content data (content type {}, title {title_id:016x})",
                header[0x205]
            ));
        }
        return Ok((
            vec![Title {
                path: path.clone(),
                title_id,
                version: 0,
                member: files[0].clone(),
                files: files.clone(),
            }],
            vec![],
        ));
    }
    let by_name: BTreeMap<String, &Member> = files
        .iter()
        .map(|m| (m.name.to_ascii_lowercase(), m))
        .collect();
    let (mut titles, mut others, mut locked) = (vec![], vec![], 0);
    for m in &files {
        if !m.name.to_ascii_lowercase().ends_with(".cnmt.nca") || m.size > 16 << 20 {
            continue;
        }
        let meta = Nca::open(&blob, m, keys, &Tickets::new())
            .ok()
            .and_then(|n| {
                let s = n.sections.first()?;
                pfs_files(&n, s)
                    .into_iter()
                    .find(|(k, _)| k.to_ascii_lowercase().ends_with(".cnmt"))
                    .map(|x| x.1)
            });
        let Some(c) = meta.and_then(|d| cnmt::parse(&d).ok()) else {
            locked += 1;
            continue;
        };
        if c.kind != cnmt::ADD_ON {
            others.push(c.kind);
            continue;
        }
        let holder = c.records.iter().filter(|r| r.kind == 2).find_map(|r| {
            by_name
                .get(&format!("{}.nca", r.nca_id))
                .or_else(|| by_name.get(&format!("{}.ncz", r.nca_id)))
        });
        let Some(holder) = holder else {
            return fmt(format!(
                "{p}: add-on content {:016x} lists data that is not in the package",
                c.title_id
            ));
        };
        titles.push(Title {
            path: path.clone(),
            title_id: c.title_id,
            version: c.version,
            member: (*holder).clone(),
            files: files.clone(),
        });
    }
    if titles.is_empty() && locked > 0 {
        return fmt(format!(
            "{p}: the installed keys do not open its content metadata (newer keys are needed)"
        ));
    }
    titles.sort_by_key(|t| t.title_id);
    Ok((titles, others))
}

/// (offset in the NCA, size, section offset, section index, crypto) of the IVFC RomFS data level.
fn romfs_range(header: &[u8], p: &str) -> Result<(u64, u64, u64, u64, u8, Vec<u8>)> {
    for index in 0..4 {
        let (start, end) = (
            u32le(header, 0x240 + index * 0x10) as u64,
            u32le(header, 0x244 + index * 0x10) as u64,
        );
        if start == 0 && end == 0 {
            continue;
        }
        let fs = &header[0x400 + index * 0x200..0x600 + index * 0x200];
        if fs[3] != 3 || &fs[8..12] != b"IVFC" {
            continue;
        }
        let (data_at, data_size) = (u64le(fs, 0x18 + 5 * 0x18), u64le(fs, 0x20 + 5 * 0x18));
        let (section_at, section_size) =
            (start * 0x200, end.wrapping_sub(start).wrapping_mul(0x200));
        if end < start
            || data_at > section_size
            || data_size > section_size - data_at
            || data_size < 0x50
        {
            return fmt(format!(
                "{p}: the RomFS range of the add-on content is out of bounds"
            ));
        }
        return Ok((
            section_at + data_at,
            data_size,
            section_at,
            section_size,
            fs[4],
            fs.to_vec(),
        ));
    }
    fmt(format!("{p}: the add-on content has no RomFS"))
}

fn romfs_head_ok(head: &[u8], size: u64) -> bool {
    head.len() >= 0x50
        && u64le(head, 0) == 0x50
        && [3, 7, 9].iter().all(|&i| u64le(head, i * 8) <= size)
}

fn romfs_file_ok(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut head = vec![0u8; 0x50];
    f.read_exact(&mut head).is_ok() && romfs_head_ok(&head, size)
}

/// dlc.py `ncz_plain`: the NCA an .ncz holds, from 0x4000 on, as stored (section data decrypted), as a reader.
fn ncz_reader(blob: &Blob, member: &Member) -> Result<Box<dyn Read>> {
    let p = blob.path.display();
    let name = &member.name;
    let mut at = member.offset + 0x4000;
    let end = member.offset + member.size;
    if member.size < 0x4010 || blob.read_at(at, 8)? != b"NCZSECTN" {
        return fmt(format!("{p}: {name} is not an NCZ"));
    }
    let count = u64le(&blob.read_at(at + 8, 8)?, 0);
    if count > 0x10000 || at + 16 + count * 0x40 + 8 > end {
        return fmt(format!("{p}: {name} has an invalid NCZ section table"));
    }
    at += 16 + count * 0x40;
    if blob.read_at(at, 8)? == b"NCZBLOCK" {
        let h = blob.read_at(at + 8, 16)?;
        let (exponent, blocks, total) = (h[3] as u32, u32le(&h, 4) as u64, u64le(&h, 8));
        if !(14..=32).contains(&exponent) || !(1..=1 << 24).contains(&blocks) {
            return fmt(format!("{p}: {name} uses an unknown NCZ block format"));
        }
        let sizes_raw = blob.read_at(at + 24, 4 * blocks)?;
        let sizes: Vec<u64> = (0..blocks as usize)
            .map(|i| u32le(&sizes_raw, 4 * i) as u64)
            .collect();
        let block = 1u64 << exponent;
        let mut pos = at + 24 + 4 * blocks;
        if pos + sizes.iter().sum::<u64>() > end || total > blocks * block {
            return fmt(format!("{p}: {name} has NCZ blocks out of range"));
        }
        let mut out = vec![];
        for (i, &stored) in sizes.iter().enumerate() {
            let want = block.min(total.saturating_sub(i as u64 * block));
            let data = blob.read_at(pos, stored)?;
            pos += stored;
            let data = if stored < want {
                zstd::bulk::decompress(&data, want as usize).map_err(|_| {
                    PackageError::Format(format!("{p}: {name} has invalid zstd data"))
                })?
            } else {
                data
            };
            if data.len() as u64 != want {
                return fmt(format!("{p}: {name} has an NCZ block of the wrong size"));
            }
            out.extend(data);
        }
        return Ok(Box::new(std::io::Cursor::new(out)));
    }
    let src = blob.reader(at, end - at)?;
    let dec = zstd::stream::read::Decoder::new(src)
        .map_err(|_| PackageError::Format(format!("{p}: {name} has invalid zstd data")))?;
    Ok(Box::new(dec))
}

/// dlc.py `extract`, in place: put the title's RomFS in `folder`. Returns "package" (a marker, read in place) or
/// "image" (romfs.bin unpacked from an NCZ).
pub fn extract(
    keys: &Keys,
    title: &Title,
    folder: &Path,
    say: &dyn Fn(&str),
) -> Result<&'static str> {
    let p = title.path.display().to_string();
    let blob = Blob::open(&title.path)?;
    std::fs::create_dir_all(folder).map_err(|source| PackageError::Io {
        path: folder.to_path_buf(),
        source,
    })?;
    for name in ["romfs.nsp", "romfs.bin", "romfs.bin.tmp", "content.nca.tmp"] {
        let _ = std::fs::remove_file(folder.join(name));
    }
    let member = &title.member;
    let header = decrypt_header(&blob, member.offset, keys)
        .ok()
        .filter(|h| matches!(&h[0x200..0x204], b"NCA3" | b"NCA2"));
    let Some(header) = header else {
        return fmt(format!(
            "{p}: the installed keys do not open the data of {:016x}",
            title.title_id
        ));
    };
    let packed = member.name.to_ascii_lowercase().ends_with(".ncz");
    let (data_at, data_size, section_at, section_size, crypto, fs) = romfs_range(&header, &p)?;
    let generation = header[0x206].max(header[0x220]).saturating_sub(1);
    let undecrypted = || {
        PackageError::Format(format!(
            "{p}: the data of {:016x} did not decrypt to a RomFS; the installed keys probably lack this content's \
             generation (titlekek_{generation:02x} / key_area_key_application_{generation:02x})",
            title.title_id
        ))
    };
    let mb = data_size as f64 / 1048576.0;
    if packed {
        say(&format!(
            "Unpacking {:016x} ({mb:.1} MB)...",
            title.title_id
        ));
        if data_at < 0x4000 {
            return fmt(format!(
                "{p}: the RomFS of {:016x} starts inside the NCZ header",
                title.title_id
            ));
        }
        let image = folder.join("romfs.bin");
        let partial = folder.join("romfs.bin.tmp");
        let mut r = ncz_reader(&blob, member)?;
        std::io::copy(&mut (&mut r).take(data_at - 0x4000), &mut std::io::sink()).map_err(
            |_| PackageError::Format(format!("{p}: {} has invalid zstd data", member.name)),
        )?;
        let mut out = std::fs::File::create(&partial).map_err(|source| PackageError::Io {
            path: partial.clone(),
            source,
        })?;
        let n = std::io::copy(&mut r.take(data_size), &mut out).map_err(|_| {
            PackageError::Format(format!("{p}: {} has invalid zstd data", member.name))
        })?;
        drop(out);
        if n < data_size {
            let _ = std::fs::remove_file(&partial);
            return fmt(format!(
                "{p}: the NCZ of {:016x} ends before its RomFS",
                title.title_id
            ));
        }
        if !romfs_file_ok(&partial) {
            let _ = std::fs::remove_file(&partial);
            return Err(undecrypted());
        }
        std::fs::rename(&partial, &image).map_err(|source| PackageError::Io {
            path: image.clone(),
            source,
        })?;
        return Ok("image");
    }
    let marker_path = folder.join(marker::NSP);
    let write = |text: String| -> Result<()> {
        std::fs::write(&marker_path, text).map_err(|source| PackageError::Io {
            path: marker_path.clone(),
            source,
        })
    };
    match crypto {
        1 => {
            write(format!(
                "SWRNSP1\n{p}\n{:x}\n{:x}\n",
                member.offset + data_at,
                data_size
            ))?;
            Ok("package")
        }
        3 => {
            let rights: [u8; 16] = header[0x230..0x240].try_into().unwrap();
            let (name, wrapped) = if rights.iter().any(|&b| b != 0) {
                let mut blobs = vec![];
                for t in title
                    .files
                    .iter()
                    .filter(|m| m.name.to_ascii_lowercase().ends_with(".tik") && m.size <= 0x10000)
                {
                    blobs.push(blob.read_at(t.offset, t.size)?);
                }
                let beside = title
                    .path
                    .parent()
                    .map(|d| d.join(format!("{}.tik", hex(&rights))));
                if container::extension(&title.path) == ".nca" {
                    if let Some(b) = beside.filter(|b| b.is_file()) {
                        let mut v = std::fs::read(&b).unwrap_or_default();
                        v.truncate(0x10000);
                        blobs.push(v);
                    }
                }
                let (mut key, mut bound) = (None, false);
                for b in &blobs {
                    if let Some((r, value)) = ticket_key(b) {
                        if r == rights {
                            key = value;
                            bound = value.is_none();
                        }
                    }
                }
                if bound {
                    return fmt(format!(
                        "{p}: the ticket of {:016x} is bound to one console (a personalized ticket); its content key \
                         cannot be read here. Dump the add-on content with a common ticket.",
                        title.title_id
                    ));
                }
                let Some(key) = key else {
                    return fmt(format!(
                        "{p}: the data of {:016x} needs a ticket (.tik) and the package has none for it",
                        title.title_id
                    ));
                };
                (format!("titlekek_{generation:02x}"), key)
            } else {
                let area = ["application", "ocean", "system"]
                    .get(header[0x207] as usize)
                    .copied()
                    .unwrap_or("application");
                (
                    format!("key_area_key_{area}_{generation:02x}"),
                    header[0x320..0x330].try_into().unwrap(),
                )
            };
            say(&format!(
                "Reading {:016x} in place ({mb:.1} MB, decrypted per read)...",
                title.title_id
            ));
            let kek = keys.get16(&name).map_err(|_| undecrypted())?;
            let mut upper: [u8; 8] = fs[0x140..0x148].try_into().unwrap();
            upper.reverse();
            let section = SectionStorage {
                blob: Arc::new(Blob::open(&title.path)?),
                file_offset: member.offset + section_at,
                size: section_size,
                nca_offset: section_at,
                crypto: 3,
                upper,
                key: Some(ecb_decrypt(&kek, &wrapped)[..].try_into().unwrap()),
                fs: fs.clone(),
            };
            let head = section.read(data_at - section_at, 0x50)?;
            if !romfs_head_ok(&head, data_size) {
                return Err(undecrypted());
            }
            write(format!(
                "SWRNSP2\n{p}\nkeys {}\nromfs {:x} {data_size:x}\nbase {:x} {section_size:x} {section_at:x} 3 {} {name}:{}\n",
                keys.path.display(),
                data_at - section_at,
                member.offset + section_at,
                hex(&upper),
                hex(&wrapped)
            ))?;
            Ok("package")
        }
        c => fmt(format!(
            "{p}: the data of {:016x} uses an encryption the installer does not read (type {c})",
            title.title_id
        )),
    }
}

// ------------------------------------------------------------------ the manifest

pub fn manifest_path(datadir: &Path) -> PathBuf {
    datadir.join("dlc.manifest")
}

pub fn read_manifest(datadir: &Path) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(manifest_path(datadir)) else {
        return vec![];
    };
    let mut out = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line
            .splitn(3, char::is_whitespace)
            .filter(|s| !s.is_empty());
        let (Some(id), Some(ver)) = (parts.next(), parts.next()) else {
            continue;
        };
        if id.len() != 16 {
            continue;
        }
        let (Ok(id), Ok(ver)) = (u64::from_str_radix(id, 16), ver.parse::<u32>()) else {
            continue;
        };
        let rest = line
            .splitn(2, char::is_whitespace)
            .nth(1)
            .unwrap_or("")
            .trim_start();
        let rest = rest
            .splitn(2, char::is_whitespace)
            .nth(1)
            .unwrap_or("")
            .trim_start();
        let (kind, rest) = match rest.split_once(char::is_whitespace) {
            Some((w, r)) if w == "data" || w == "stub" => (w.to_string(), r),
            _ if rest == "data" || rest == "stub" => (rest.to_string(), ""),
            _ => ("data".to_string(), rest),
        };
        out.push((id, ver, kind, rest.trim().to_string()));
    }
    out
}

pub fn write_manifest(datadir: &Path, entries: &[Entry]) -> Result<()> {
    let path = manifest_path(datadir);
    if entries.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let mut entries = entries.to_vec();
    entries.sort();
    let mut text =
        String::from("# SwitchRecomp add-on content: <title id> <version> <data|stub> <package>\n");
    for (id, ver, kind, src) in &entries {
        text += &format!("{id:016x} {ver} {kind} {src}\n");
    }
    let tmp = path.with_extension("manifest.tmp");
    std::fs::write(&tmp, text).map_err(|source| PackageError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, &path).map_err(|source| PackageError::Io {
        path: path.clone(),
        source,
    })
}

pub fn title_folder(datadir: &Path, title_id: u64) -> PathBuf {
    datadir.join("dlc").join(format!("{title_id:016x}"))
}

/// dlc.py `same_file`: the same path after abspath and (on Windows) case folding.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = abspath(p).display().to_string();
        if cfg!(windows) {
            s.replace('/', "\\").to_lowercase()
        } else {
            s
        }
    };
    norm(a) == norm(b)
}

/// The game's title id from `exefs/main.npdm` (the ACI0 program id).
pub fn game_title_id(datadir: &Path) -> Result<u64> {
    let d = std::fs::read(datadir.join("exefs").join("main.npdm")).map_err(|_| {
        PackageError::Format(format!(
            "{} is not a built game's data folder (build the game first)",
            datadir.display()
        ))
    })?;
    let default = 0x0100_0000_0000_1000;
    if d.len() < 0x78 || &d[..4] != b"META" {
        return Ok(default);
    }
    let aci = u32le(&d, 0x70) as usize;
    if d.get(aci..aci + 4) == Some(b"ACI0") && aci + 0x18 <= d.len() {
        return Ok(u64le(&d, aci + 0x10));
    }
    Ok(default)
}

fn stamp(p: &Path) -> Option<(String, u64, i128)> {
    let m = std::fs::metadata(p).ok()?;
    let mtime = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or_else(|e| -(e.duration().as_nanos() as i128));
    Some((abspath(p).display().to_string(), m.len(), mtime))
}

/// Python `json.dumps(s)` of a string (ensure_ascii).
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out += &format!("\\u{u:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out + "\""
}

/// install.json as dlc.py writes it (`json.dump(state, indent=1)`).
fn install_json(source: &str, size: u64, mtime: i128, storage: &str, version: u32) -> String {
    format!(
        "{{\n \"source\": {},\n \"size\": {size},\n \"mtime_ns\": {mtime},\n \"storage\": {},\n \"version\": {version}\n}}",
        json_str(source),
        json_str(storage)
    )
}

/// True when this title is already installed from this very file.
fn installed(datadir: &Path, title_id: u64, source: &Path) -> bool {
    let folder = title_folder(datadir, title_id);
    let Ok(text) = std::fs::read_to_string(folder.join("install.json")) else {
        return false;
    };
    let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some((_, size, mtime)) = stamp(source) else {
        return false;
    };
    if state.get("size").and_then(|v| v.as_u64()) != Some(size)
        || state
            .get("mtime_ns")
            .and_then(|v| v.as_i64())
            .map(|v| v as i128)
            != Some(mtime)
        || !same_file(
            Path::new(state.get("source").and_then(|v| v.as_str()).unwrap_or("")),
            source,
        )
    {
        return false;
    }
    match state.get("storage").and_then(|v| v.as_str()) {
        Some("image") => romfs_file_ok(&folder.join("romfs.bin")),
        Some("package") => folder.join("romfs.nsp").is_file(),
        Some("stub") => true,
        _ => false,
    }
}

fn has_data(datadir: &Path, title_id: u64) -> bool {
    let f = title_folder(datadir, title_id);
    f.join("romfs.nsp").is_file() || romfs_file_ok(&f.join("romfs.bin"))
}

fn remove_empty_root(datadir: &Path) {
    let root = datadir.join("dlc");
    if std::fs::read_dir(&root)
        .map(|mut r| r.next().is_none())
        .unwrap_or(false)
    {
        let _ = std::fs::remove_dir(&root);
    }
}

/// Install the add-on content of each package for the game built in `datadir`. Returns (installed ids, errors);
/// every refusal is also said as `DLC error: <reason>`.
pub fn install(
    paths: &[PathBuf],
    datadir: &Path,
    say: &dyn Fn(&str),
    keys: Option<&Path>,
    allow_stub: bool,
) -> Result<(Vec<u64>, Vec<String>)> {
    let datadir = abspath(datadir);
    let application = game_title_id(&datadir)?;
    let keys = Keys::load(&keys_path(keys)?)?;
    let mut entries: BTreeMap<u64, Entry> = read_manifest(&datadir)
        .into_iter()
        .map(|e| (e.0, e))
        .collect();
    let (mut done, mut errors) = (vec![], vec![]);
    for path in paths {
        let path = abspath(path);
        let result = (|| -> Result<()> {
            let (titles, others) = scan(&path, &keys)?;
            let mine: Vec<&Title> = titles
                .iter()
                .filter(|t| is_addon_of(t.title_id, application))
                .collect();
            if mine.is_empty() {
                if let Some(t) = titles.first() {
                    return fmt(format!(
                        "{}: this add-on content is for {:016x}, not for this game ({application:016x})",
                        path.display(),
                        base_of(t.title_id)
                    ));
                }
                let mut held: Vec<&str> = others.iter().map(|&k| cnmt::type_name(k)).collect();
                held.sort();
                held.dedup();
                let held = held.join(", ");
                return fmt(format!(
                    "{}: the package has no add-on content{}",
                    path.display(),
                    if held.is_empty() {
                        String::new()
                    } else {
                        format!(" (it holds {held})")
                    }
                ));
            }
            for t in mine {
                match install_title(&keys, t, &datadir, &mut entries, say, allow_stub) {
                    Ok(id) => done.push(id),
                    Err(e) => {
                        errors.push(e.to_string());
                        say(&format!("DLC error: {e}"));
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            errors.push(e.to_string());
            say(&format!("DLC error: {e}"));
        }
    }
    Ok((done, errors))
}

fn install_title(
    keys: &Keys,
    title: &Title,
    datadir: &Path,
    entries: &mut BTreeMap<u64, Entry>,
    say: &dyn Fn(&str),
    allow_stub: bool,
) -> Result<u64> {
    let path = &title.path;
    let folder = title_folder(datadir, title.title_id);
    if installed(datadir, title.title_id, path) && entries.contains_key(&title.title_id) {
        say(&format!(
            "DLC: {:016x} v{} is already installed",
            title.title_id, title.version
        ));
        return Ok(title.title_id);
    }
    if let Some(have) = entries.get(&title.title_id) {
        if !have.3.is_empty()
            && !same_file(Path::new(&have.3), path)
            && have.1 >= title.version
            && has_data(datadir, title.title_id)
        {
            let base = Path::new(&have.3)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            return fmt(format!(
                "{}: {:016x} v{} is already installed from {base} (remove that one first to install it from this file)",
                path.display(),
                title.title_id,
                title.version
            ));
        }
    }
    let mut kind = "data";
    let storage = match extract(keys, title, &folder, say) {
        Ok(s) => s,
        Err(e) => {
            if !allow_stub {
                let _ = std::fs::remove_dir_all(&folder);
                if entries.remove(&title.title_id).is_some() {
                    write_manifest(datadir, &entries.values().cloned().collect::<Vec<_>>())?;
                }
                remove_empty_root(datadir);
                return Err(e);
            }
            say(&format!("DLC warning: {e}"));
            std::fs::create_dir_all(&folder).map_err(|source| PackageError::Io {
                path: folder.clone(),
                source,
            })?;
            kind = "stub";
            "stub"
        }
    };
    let (source, size, mtime) = stamp(path).ok_or_else(|| {
        PackageError::Format(format!("{}: the package is missing", path.display()))
    })?;
    let json = install_json(&source, size, mtime, storage, title.version);
    std::fs::write(folder.join("install.json"), json).map_err(|source| PackageError::Io {
        path: folder.clone(),
        source,
    })?;
    entries.insert(
        title.title_id,
        (
            title.title_id,
            title.version,
            kind.to_string(),
            path.display().to_string(),
        ),
    );
    write_manifest(datadir, &entries.values().cloned().collect::<Vec<_>>())?;
    let what = match storage {
        "package" => "read in place from the package".to_string(),
        "image" => format!(
            "RomFS image, {:.1} KB",
            std::fs::metadata(folder.join("romfs.bin"))
                .map(|m| m.len())
                .unwrap_or(0) as f64
                / 1024.0
        ),
        _ => "license only, no data: the game may not accept it".to_string(),
    };
    say(&format!(
        "DLC: installed {:016x} v{} ({what}) from {}",
        title.title_id,
        title.version,
        path.display()
    ));
    Ok(title.title_id)
}

/// Remove titles by id (16 hex digits) or by the package they came from. Returns the number removed.
pub fn remove(datadir: &Path, what: &str, say: &dyn Fn(&str)) -> Result<usize> {
    let datadir = abspath(datadir);
    let entries = read_manifest(&datadir);
    let mut keep = vec![];
    for e in &entries {
        let matched = format!("{:016x}", e.0) == what.to_lowercase()
            || (!e.3.is_empty() && same_file(Path::new(&e.3), Path::new(what)));
        if !matched {
            keep.push(e.clone());
            continue;
        }
        let _ = std::fs::remove_dir_all(title_folder(&datadir, e.0));
        say(&format!("DLC: removed {:016x}", e.0));
    }
    write_manifest(&datadir, &keep)?;
    remove_empty_root(&datadir);
    Ok(entries.len() - keep.len())
}

/// Make the installed add-on content exactly that of these packages. Returns (installed ids, errors).
pub fn sync(
    paths: &[PathBuf],
    datadir: &Path,
    say: &dyn Fn(&str),
    keys: Option<&Path>,
) -> Result<(Vec<u64>, Vec<String>)> {
    let datadir = abspath(datadir);
    let wanted: Vec<PathBuf> = paths.iter().map(|p| abspath(p)).collect();
    for e in read_manifest(&datadir) {
        if !wanted
            .iter()
            .any(|w| !e.3.is_empty() && same_file(Path::new(&e.3), w))
        {
            remove(&datadir, &format!("{:016x}", e.0), say)?;
        }
    }
    let mut todo = vec![];
    for w in &wanted {
        let current: Vec<Entry> = read_manifest(&datadir)
            .into_iter()
            .filter(|e| !e.3.is_empty() && same_file(Path::new(&e.3), w))
            .collect();
        if !current.is_empty() && !w.is_file() {
            for e in &current {
                if title_folder(&datadir, e.0).join("romfs.nsp").is_file() {
                    say(&format!(
                        "DLC warning: {:016x} is read from {}, which is missing",
                        e.0,
                        w.display()
                    ));
                }
            }
            continue;
        }
        if !current.is_empty() && current.iter().all(|e| installed(&datadir, e.0, w)) {
            continue;
        }
        todo.push(w.clone());
    }
    if todo.is_empty() {
        return Ok((
            read_manifest(&datadir).into_iter().map(|e| e.0).collect(),
            vec![],
        ));
    }
    install(&todo, &datadir, say, keys, false)
}

/// List the installed add-on content (`<id> v<version> <kind> <package>` lines).
pub fn describe(datadir: &Path, say: &dyn Fn(&str)) -> Vec<Entry> {
    let entries = read_manifest(datadir);
    for (id, ver, kind, src) in &entries {
        say(&format!("{id:016x} v{ver} {kind} {src}"));
    }
    if entries.is_empty() {
        say("No add-on content is installed.");
    }
    entries
}

/// The RomFS of an installed title, as the runtime mounts it (`OpenDataStorageByDataId`): `romfs.nsp`, then
/// `romfs.bin`, then a hand-filled `romfs/` folder.
pub fn title_romfs(datadir: &Path, title_id: u64, keys: Option<&Path>) -> Result<RomFs> {
    let folder = title_folder(datadir, title_id);
    let m = folder.join(marker::NSP);
    if m.is_file() {
        return marker::open_nsp_marker(&m, keys);
    }
    let bin = folder.join("romfs.bin");
    if bin.is_file() {
        let blob = Arc::new(Blob::open(&bin)?);
        let size = blob.size;
        let image = Slice {
            inner: Arc::new(crate::package::storage::FileRange {
                blob,
                offset: 0,
                size,
            }),
            offset: 0,
            size,
        };
        return RomFs::from_image(Arc::new(image), format!("image {}", bin.display()));
    }
    RomFs::from_folder(&folder.join("romfs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip_and_forms() {
        let d = std::env::temp_dir().join(format!("nxrs-dlc-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            manifest_path(&d),
            "# comment\n0100152000023001 65536 data P:\\Games\\a b.nsp\n0100152000023002 1 Q:\\c.nsp\nbad line\n0100152000023003 2 stub\n",
        )
        .unwrap();
        let e = read_manifest(&d);
        assert_eq!(
            e[0],
            (
                0x0100152000023001,
                65536,
                "data".into(),
                "P:\\Games\\a b.nsp".into()
            )
        );
        assert_eq!(
            e[1],
            (0x0100152000023002, 1, "data".into(), "Q:\\c.nsp".into())
        );
        assert_eq!(e[2], (0x0100152000023003, 2, "stub".into(), "".into()));
        write_manifest(&d, &[e[1].clone(), e[0].clone()]).unwrap();
        let text = std::fs::read_to_string(manifest_path(&d)).unwrap();
        assert_eq!(
            text,
            "# SwitchRecomp add-on content: <title id> <version> <data|stub> <package>\n\
             0100152000023001 65536 data P:\\Games\\a b.nsp\n0100152000023002 1 data Q:\\c.nsp\n"
        );
        write_manifest(&d, &[]).unwrap();
        assert!(!manifest_path(&d).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn install_json_like_python() {
        assert_eq!(
            install_json("P:\\Pokémon\\a.nsp", 3, 5, "package", 7),
            "{\n \"source\": \"P:\\\\Pok\\u00e9mon\\\\a.nsp\",\n \"size\": 3,\n \"mtime_ns\": 5,\n \"storage\": \"package\",\n \"version\": 7\n}"
        );
    }

    #[test]
    fn npdm_title_id() {
        let d = std::env::temp_dir().join(format!("nxrs-npdm-{}", std::process::id()));
        std::fs::create_dir_all(d.join("exefs")).unwrap();
        assert!(game_title_id(&d)
            .unwrap_err()
            .to_string()
            .contains("is not a built game's data folder"));
        let mut n = vec![0u8; 0x100];
        n[..4].copy_from_slice(b"META");
        n[0x70..0x74].copy_from_slice(&0x80u32.to_le_bytes());
        n[0x80..0x84].copy_from_slice(b"ACI0");
        n[0x90..0x98].copy_from_slice(&0x0100152000022000u64.to_le_bytes());
        std::fs::write(d.join("exefs/main.npdm"), &n).unwrap();
        assert_eq!(game_title_id(&d).unwrap(), 0x0100152000022000);
        let _ = std::fs::remove_dir_all(&d);
    }
}

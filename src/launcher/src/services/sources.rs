//! What can be added to the library: a source package or folder, or a native build on disk.

use super::files::{is_dir, is_file, read_at};
use super::markers::{adopt_built_update, build_cover, source_of_build};
use crate::constants::{engine as e, words_library as w};
use crate::model::build_layout::build_dir_for;
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::names::{output_name_for, title_from_name};
use crate::model::path_text::{absolute, base_name, ends_with_ci, file_name, join, parent, suffix};

fn at(bytes: &[u8], i: usize, what: &[u8]) -> bool {
    bytes.get(i..i + what.len()) == Some(what)
}

fn u32_le(bytes: &[u8], i: usize) -> Option<u32> {
    let b = bytes.get(i..i + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u16_be(bytes: &[u8], i: usize) -> Option<u16> {
    let b = bytes.get(i..i + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn u64_be(bytes: &[u8], i: usize) -> Option<u64> {
    let b = bytes.get(i..i + 8)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

/// An extracted folder (`main`, `exefs/main`, `ExeFS/main`), an indexed NUP, an application NSP, a
/// card image (XCI), an NRO, or a decrypted 3DS dump (`.3ds`, `.cia`, `.cxi`/`.app`).
pub fn valid_source(source: &str) -> Result<(), String> {
    if is_dir(source) {
        if ["main", "exefs/main", "ExeFS/main"]
            .iter()
            .any(|n| is_file(&join(source, n)))
        {
            return Ok(());
        }
        return Err(w::NO_MAIN.into());
    }
    let head = read_at(source, 0, 0x1104)?;
    match suffix(source).as_str() {
        "nup" if head.starts_with(b"NUP\x01") => Ok(()),
        "nsp" if head.starts_with(b"PFS0") => Ok(()),
        "nro" if at(&head, 0x10, b"NRO0") => Ok(()),
        // a card image: "HEAD" at 0x100, after the 0x1000-byte key area when the dump kept it
        "xci" if at(&head, 0x100, b"HEAD") || at(&head, 0x1100, b"HEAD") => Ok(()),
        "xci" => Err(fill(w::NOT_CARD, &[&file_name(source)])),
        "3ds" => cart(source),
        "cia" => cia(source),
        // the game's NCCH partition alone: its header starts at 0 (the RSA signature occupies the
        // first 0x100 bytes, the magic reads at +0x100); a .cxi and a bare content .app are the
        // same bytes
        "cxi" | "app" => ncch(source, 0, w::NOT_CXI),
        _ => Err(w::NOT_SOURCE.into()),
    }
}

/// The NCCH at `at`: its magic, and no encryption the engine would refuse (a crypto method other
/// than none without the NoCrypto flag).
fn ncch(source: &str, at: u64, missing: &str) -> Result<(), String> {
    let head = read_at(source, at, 0x190)?;
    if head.get(0x100..0x104) != Some(b"NCCH") {
        return Err(fill(missing, &[&file_name(source)]));
    }
    let method = head.get(0x18B).copied().unwrap_or(0);
    let flags = head.get(0x18F).copied().unwrap_or(0);
    if method != 0 && flags & 0x04 == 0 {
        return Err(fill(w::ENCRYPTED_3DS, &[&file_name(source)]));
    }
    Ok(())
}

/// A .3ds cart image: NCSD at 0x100, the game's NCCH as partition 0 (the table at +0x120,
/// media units of 0x200).
fn cart(source: &str) -> Result<(), String> {
    let head = read_at(source, 0, 0x124)?;
    if head.get(0x100..0x104) != Some(b"NCSD") || u32_le(&head, 0x120).is_none() {
        return Err(fill(w::NOT_CART, &[&file_name(source)]));
    }
    let partition = u32_le(&head, 0x120).unwrap_or(0) as u64 * 0x200;
    ncch(source, partition, w::NOT_CART)
}

/// A .cia: the 0x2020-byte header, then the TMD, whose content record 0 must name a game (not an
/// update or add-on title) and be plain (the encrypted bit clear).
fn cia(source: &str) -> Result<(), String> {
    let head = read_at(source, 0, 0x2020)?;
    if u32_le(&head, 0) != Some(0x2020) {
        return Err(fill(w::NOT_CIA, &[&file_name(source)]));
    }
    let align = |n: u64| n.next_multiple_of(0x40);
    let tmd = align(
        align(align(0x2020) + u64::from(u32_le(&head, 8).unwrap_or(0)))
            + u64::from(u32_le(&head, 0xC).unwrap_or(0)),
    );
    // the TMD from its signature type on: the signature's size, then the title id, the content
    // count and the first record, which sits after the 64 content-chunk records (64 x 0x24)
    let raw = read_at(
        source,
        tmd,
        0xB34.min(u32_le(&head, 0x10).unwrap_or(0) as usize),
    )?;
    let refuse = || fill(w::NOT_CIA, &[&file_name(source)]);
    // the signature type's low half selects its size, padded to 64
    let sig = match u16_be(&raw, 2).unwrap_or(0) {
        0x0000 | 0x0003 => 0x240,
        0x0001 | 0x0004 => 0x140,
        0x0002 | 0x0005 => 0x80,
        _ => return Err(refuse()),
    };
    let record = sig + 0xC4 + 64 * 0x24;
    if raw.len() < record + 0x30 {
        return Err(refuse());
    }
    match u64_be(&raw, sig + 0x4C).unwrap_or(0) >> 32 {
        0x0004_000E | 0x0004_008C => return Err(fill(w::NOT_GAME_CIA, &[&file_name(source)])),
        _ => {}
    }
    if u16_be(&raw, sig + 0x9E) == Some(0) || u16_be(&raw, record + 4) != Some(0) {
        return Err(refuse());
    }
    if head.get(0x20).is_none_or(|b| b & 0x80 == 0) {
        return Err(refuse());
    }
    if u16_be(&raw, record + 6).is_some_and(|kind| kind & 1 != 0) {
        return Err(fill(w::ENCRYPTED_3DS, &[&file_name(source)]));
    }
    Ok(())
}

/// The title of a source: its folder or file name, tags removed.
pub fn title_for_source(source: &str) -> String {
    title_from_name(&if is_dir(source) {
        file_name(source)
    } else {
        base_name(source)
    })
}

/// A new entry for a source: the nxrs engine's, built into `<source folder>\nxrs\<name>` (or
/// `<Build location>\nxrs\<name>` when it is set).
pub fn game_from_source(source: &str, output_root: &str) -> Game {
    let title = title_for_source(source);
    let output_name = output_name_for(&title);
    Game {
        id: uuid::Uuid::new_v4().to_string(),
        source: absolute(source),
        build_dir: build_dir_for(&absolute(source), output_root, &output_name),
        engine: e::ENGINE_NXRS.into(),
        title,
        output_name,
        ..Game::default()
    }
}

/// A native build on disk: its executable, code module and data folder; source, update, renderer and
/// cover read from what is there.
pub fn game_from_executable(path: &str) -> Result<Game, String> {
    if !is_file(path) {
        return Err(w::NO_EXECUTABLE.into());
    }
    let mut title = file_name(path);
    if !e::EXECUTABLE_SUFFIX.is_empty() && ends_with_ci(&title, e::EXECUTABLE_SUFFIX) {
        title.truncate(title.len() - e::EXECUTABLE_SUFFIX.len());
    }
    let mut g = Game {
        id: uuid::Uuid::new_v4().to_string(),
        output_name: title.clone(),
        title,
        build_dir: parent(path),
        ..Game::default()
    };
    if !super::build_facts::has_build(&g) {
        return Err(w::INCOMPLETE_BUILD.into());
    }
    g.source = source_of_build(&g.data_dir());
    adopt_built_update(&mut g);
    g.built_source = g.source.clone();
    g.renderer = super::ini_file::read_renderer(&g.ini_path());
    g.cover = build_cover(&g.data_dir());
    Ok(g)
}

//! The 3DS sources (`threeDsSourceClassification`, the .cia refusals) and the SMDH card art
//! (`smdhDecodesIconAndTitles`), plus the two real dumps of the wave when they are on disk —
//! service-layer only, against a temp library, never the real one.

use super::library_ops::add;
use super::services_tests::{tmp, write};
use super::sources::{game_from_source, valid_source};
use crate::model::library::Library;

/// an SMDH: a first-language title (short, long, publisher) with an English entry beside it, and
/// a 48x48 RGB565 icon of one colour
fn smdh(title: (&str, &str, &str), english: Option<(&str, &str, &str)>, pixel: u16) -> Vec<u8> {
    let utf16 =
        |text: &str| -> Vec<u8> { text.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let mut out = vec![0u8; 0x36C0];
    out[..4].copy_from_slice(b"SMDH");
    let put = |out: &mut [u8], at: usize, t: (&str, &str, &str)| {
        for (slot, text) in [(at, t.0), (at + 0x40, t.1), (at + 0xC0, t.2)] {
            let bytes = utf16(text);
            out[slot..slot + bytes.len()].copy_from_slice(&bytes);
        }
    };
    put(&mut out, 0x10, title);
    if let Some(english) = english {
        put(&mut out, 0x210, english);
    }
    let icon: Vec<u8> = (0..48 * 48).flat_map(|_| pixel.to_le_bytes()).collect();
    out[0x2480..0x2480 + icon.len()].copy_from_slice(&icon);
    out
}

/// an NCCH partition as valid_source reads it: NCCH magic at +0x100, the crypto method (flags[3])
/// and the NoCrypto flag (flags[7] bit 2) at +0x188
fn ncch(encrypted: bool) -> Vec<u8> {
    let mut out = vec![0u8; 0x290];
    out[0x100..0x104].copy_from_slice(b"NCCH");
    if encrypted {
        out[0x18B] = 2;
    } else {
        out[0x18F] = 0x04;
    }
    out
}

/// a cart image: NCSD at 0x100, the partition table naming partition 0 at 0x800, the NCCH there
fn cart(ncch: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 0x800];
    out[0x100..0x104].copy_from_slice(b"NCSD");
    out[0x120..0x124].copy_from_slice(&(0x800u32 / 0x200).to_le_bytes());
    out.extend_from_slice(ncch);
    out
}

/// the tiniest .cia: the 0x2020 header (content 0 in its bitmap), a TMD naming `title` with one
/// content record (its encrypted bit when asked), the content after it
fn cia(encrypted: bool, title: u64) -> Vec<u8> {
    let mut out = vec![0u8; 0x2020];
    out[..4].copy_from_slice(&0x2020u32.to_le_bytes());
    out[0x20] = 0x80;
    let tmd_at = 0x2040usize;
    let sig = 0x140usize;
    let record = tmd_at + sig + 0xC4 + 64 * 0x24;
    let tmd_size = record + 0x30 - tmd_at;
    let content_at = (tmd_at + tmd_size).next_multiple_of(0x40);
    out.resize(content_at + 16, 0);
    out[0x10..0x14].copy_from_slice(&(tmd_size as u32).to_le_bytes());
    out[tmd_at..tmd_at + 4].copy_from_slice(&0x0001_0004u32.to_be_bytes());
    out[tmd_at + sig + 0x4C..tmd_at + sig + 0x54].copy_from_slice(&title.to_be_bytes());
    out[tmd_at + sig + 0x9E..tmd_at + sig + 0xA0].copy_from_slice(&1u16.to_be_bytes());
    out[record + 4..record + 6].copy_from_slice(&0u16.to_be_bytes());
    let kind = u16::from(encrypted);
    out[record + 6..record + 8].copy_from_slice(&kind.to_be_bytes());
    out
}

fn named(dir: &str, name: &str, bytes: &[u8]) -> String {
    let path = format!("{dir}/{name}");
    write(&path, bytes);
    path
}

/// .cxi/.app/.3ds accept when decrypted, refuse encrypted or the wrong shape, by their own words
#[test]
fn three_ds_sources_accept_and_refuse() {
    let (_t, dir) = tmp();
    let plain = ncch(false);
    valid_source(&named(&dir, "Game.cxi", &plain)).unwrap();
    valid_source(&named(&dir, "Game.app", &plain)).unwrap();
    valid_source(&named(&dir, "Game.3ds", &cart(&plain))).unwrap();
    let locked = valid_source(&named(&dir, "Locked.cxi", &ncch(true))).unwrap_err();
    assert!(locked.contains("encrypted"), "{locked}");
    let cart_locked = valid_source(&named(&dir, "Locked.3ds", &cart(&ncch(true)))).unwrap_err();
    assert!(cart_locked.contains("encrypted"), "{cart_locked}");
    let no_ncsd = valid_source(&named(&dir, "Not a cart.3ds", &[0u8; 0x900])).unwrap_err();
    assert!(no_ncsd.contains("NCSD"), "{no_ncsd}");
    let no_ncch = valid_source(&named(&dir, "Not an exe.cxi", &[0u8; 0x290])).unwrap_err();
    assert!(no_ncch.contains("NCCH"), "{no_ncch}");
}

/// a game .cia is accepted plain and refused encrypted, wrong-shaped, empty or not a game
#[test]
fn cia_sources_accept_and_refuse() {
    let (_t, dir) = tmp();
    let game = 0x0004_0000_0FF3_DE00u64;
    valid_source(&named(&dir, "Game.cia", &cia(false, game))).unwrap();
    let locked = valid_source(&named(&dir, "Locked.cia", &cia(true, game))).unwrap_err();
    assert!(locked.contains("encrypted"), "{locked}");
    let update = valid_source(&named(
        &dir,
        "Update.cia",
        &cia(false, 0x0004_000E_0FF3_DE00),
    ))
    .unwrap_err();
    assert!(update.contains("update or add-on"), "{update}");
    let addon =
        valid_source(&named(&dir, "DLC.cia", &cia(false, 0x0004_008C_0FF3_DE00))).unwrap_err();
    assert!(addon.contains("update or add-on"), "{addon}");
    let no_header = valid_source(&named(&dir, "Not a cia.cia", &[0u8; 0x2100])).unwrap_err();
    assert!(no_header.contains(".cia"), "{no_header}");
    let mut empty = cia(false, game);
    empty[0x20] = 0;
    let no_content = valid_source(&named(&dir, "Empty.cia", &empty)).unwrap_err();
    assert!(no_content.contains(".cia"), "{no_content}");
}

/// the SMDH decodes to the 48x48 icon in RGBA, the English titles when the entry says something,
/// the first entry's otherwise; decode() dispatches on the magic
#[test]
fn smdh_decodes_icon_and_titles() {
    let (_t, dir) = tmp();
    let card = |bytes: Vec<u8>| {
        let path = named(&dir, "card.smdh", &bytes);
        super::artwork_decode::smdh(&path).unwrap()
    };
    let english = smdh(
        ("Sonic JP", "First Long", "Pub"),
        Some(("Sonic Generations", "Sonic Generations 3D", "Sega")),
        0xF800,
    );
    let (img, titles) = card(english);
    assert_eq!((img.width(), img.height()), (48, 48));
    assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(
        titles,
        vec!["Sonic Generations", "Sonic Generations 3D", "Sega"]
    );
    let first = smdh(("First", "Long", "Pub"), None, 0x07E0);
    let (img, titles) = card(first);
    assert_eq!(img.get_pixel(47, 47).0, [0, 255, 0, 255]);
    assert_eq!(titles, vec!["First", "Long", "Pub"]);
    let path = named(&dir, "bad.smdh", b"not an smdh at all");
    assert!(super::artwork_decode::decode(&path).is_err());
}

/// the wave's two real dumps import through the service layer when they are on disk (skipped
/// without them, so the default suite stays green everywhere); the entry lands in a temp library
#[test]
fn the_real_dumps_import_when_present() {
    let dumps = [
        r"P:\Games\Nintendo\DUMPS\Sonic Generations (3ds)\Sonic Generations (USA) Decrypted.3ds",
        r"P:\Games\Nintendo\DUMPS\Pokemon Moon (cia)\Pokemon Moon-USA-RF-Retail.cia",
    ];
    let (_t, dir) = tmp();
    for dump in dumps {
        if !std::path::Path::new(dump).is_file() {
            eprintln!("skipped: {dump} is not on disk");
            continue;
        }
        valid_source(dump).unwrap();
        let game = game_from_source(dump, "");
        assert!(game.nxrs(), "{}", game.title);
        assert!(
            game.title.starts_with("Sonic Generations") || game.title.starts_with("Pokemon Moon"),
            "{}",
            game.title
        );
        let mut library = Library::new(&format!("{dir}/library.json"));
        assert!(add(&mut library, game));
        assert_eq!(library.games.len(), 1);
    }
}

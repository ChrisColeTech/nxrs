//! Add-on content and updates on disk (`dlcListRoundTrip`, `dlcManifestAndState`,
//! `dlcPackagesAreChecked…`, `updatesInTheLibraryAndTheBuild`).

use super::build_facts::{facts, has_build, update_pending};
use super::dlc_files::{adopt_installed, installed, up_to_date, valid_package};
use super::library_file::{Store, load, save};
use super::services_tests::{built_game, tmp, write};
use super::sources::game_from_source;
use super::update_files::{description, valid_package as valid_update};
use crate::model::args;
use crate::model::card_state::card_state;
use crate::model::dlc_title::description as dlc_description;
use crate::model::game_json;
use crate::model::game_update::GameUpdate;
use crate::model::library::Library;
use crate::model::path_text::{native, same_path};
use crate::model::update_text::token_label;
use serde_json::json;

#[test]
fn dlc_list_round_trip() {
    let (_t, dir) = tmp();
    let file = format!("{dir}/library.json");
    let mut lib = Library::new(&file);
    let mut game = game_from_source(&format!("{dir}/Kart [v0].nsp"), "");
    game.dlc = vec![
        "P:/Games/Kart/Booster [0100152000023001][v65536].nsp".into(),
        "P:/Games/Kart/Kart (1G+1U+1D).nsp".into(),
    ];
    lib.games.push(game.clone());
    save(&lib, &mut Store::default()).unwrap();
    let mut loaded = Library::new(&file);
    load(&mut loaded, &mut Store::default()).unwrap();
    assert_eq!(loaded.games[0].dlc, game.dlc);
    let twice = game_json::from_json(
        &json!({"dlc": ["P:/a/dlc.nsp", "P:\\a\\DLC.nsp", "", 7, "P:/a/other.nsz"]}),
    );
    assert_eq!(twice.dlc.len(), if cfg!(windows) { 2 } else { 3 });
}

#[test]
fn dlc_manifest_and_state() {
    let (_t, dir) = tmp();
    let mut game = built_game(&dir);
    let booster = format!("{dir}/Booster [0100152000023001][v65536].nsp");
    let combined = format!("{dir}/Kart (1G+1U+1D).nsp");
    assert!(installed(&game.data_dir()).is_empty() && up_to_date(&game));
    game.dlc = vec![booster.clone()];
    assert!(!up_to_date(&game));
    let name = "Booster [0100152000023001][v65536].nsp";
    assert_eq!(
        dlc_description(&booster, &[], true, false),
        format!("{name} — file not found")
    );
    assert_eq!(
        dlc_description(&booster, &[], true, true),
        format!("{name} — not installed yet")
    );
    assert_eq!(
        dlc_description(&booster, &[], false, true),
        format!("{name} — installs when the game is built")
    );
    let manifest = format!(
        "# SwitchRecomp add-on content\r\n0100152000023001 65536 data {}\r\n\r\nnot a manifest line\r\n0100152000023002 3 {combined}\n0100152000023003 0 stub {combined}\n",
        native(&booster)
    );
    write(
        &format!("{}/dlc.manifest", game.data_dir()),
        manifest.as_bytes(),
    );
    let titles = installed(&game.data_dir());
    assert_eq!(titles.len(), 3);
    assert_eq!(
        (
            titles[0].title_id.as_str(),
            titles[0].version,
            titles[0].stub
        ),
        ("0100152000023001", 65536, false)
    );
    assert!(same_path(&titles[0].source, &booster) && same_path(&titles[1].source, &combined));
    assert!(!titles[1].stub && titles[2].stub && titles[2].version == 0);
    assert_eq!(
        dlc_description(&booster, &titles, true, true),
        format!("{name} — 0100152000023001 v65536")
    );
    assert_eq!(
        dlc_description(&combined, &titles, true, true),
        "Kart (1G+1U+1D).nsp — 0100152000023002 v3, 0100152000023003 (no data)"
    );
    assert!(!up_to_date(&game));
    assert!(adopt_installed(&mut game));
    assert!(game.dlc.len() == 2 && same_path(&game.dlc[1], &combined));
    assert!(!adopt_installed(&mut game) && up_to_date(&game));
    game.dlc.pop();
    assert!(!up_to_date(&game));
    let want = vec![
        "dlc".to_string(),
        "sync".into(),
        "--datadir".into(),
        native(&game.data_dir()),
        native(&booster),
    ];
    assert_eq!(args::dlc_sync(&game, "", false), want);
}

#[test]
fn dlc_packages_are_checked_before_the_engine_runs() {
    let (_t, dir) = tmp();
    let mut card = vec![0u8; 0x200];
    card[0x100..0x104].copy_from_slice(b"HEAD");
    let mut dumped = vec![0u8; 0x1200];
    dumped[0x1100..0x1104].copy_from_slice(b"HEAD");
    let good: Vec<(&str, Vec<u8>)> = vec![
        ("a.nsp", b"PFS0....".to_vec()),
        ("b.NSZ", b"PFS0....".to_vec()),
        ("c.xci", card),
        ("d.xcz", dumped),
        ("e.nca", vec![b'x'; 0xC00]),
    ];
    for (name, bytes) in &good {
        write(&format!("{dir}/{name}"), bytes);
        assert!(valid_package(&format!("{dir}/{name}")).is_ok(), "{name}");
    }
    let bad: Vec<(&str, &[u8])> = vec![
        ("notes.txt", b"PFS0"),
        ("game.nup", b"NUP\x01"),
        ("f.nsp", b"HEAD"),
        ("g.xci", b"PFS0"),
        ("h.nca", b"tiny"),
    ];
    for (name, bytes) in bad {
        write(&format!("{dir}/{name}"), bytes);
        assert!(
            !valid_package(&format!("{dir}/{name}"))
                .unwrap_err()
                .is_empty(),
            "{name}"
        );
    }
    assert!(valid_package(&format!("{dir}/missing.nsp")).is_err() && valid_package(&dir).is_err());
}

#[test]
fn updates_in_the_library_and_the_build() {
    let (_t, dir) = tmp();
    let mut game = built_game(&dir);
    game.source = format!("{dir}/Game [v0].nsp");
    game.built_source = game.source.clone();
    write(&game.source, b"PFS0");
    assert!(has_build(&game) && !update_pending(&game) && game.update_token().is_empty());
    let (older, newer) = (
        format!("{dir}/Update [v65536].nsp"),
        format!("{dir}/Update [v262144].nsp"),
    );
    write(&older, b"PFS0");
    write(&newer, b"PFS0");
    let u = |p: &str, v: i64, d: &str| GameUpdate {
        path: p.into(),
        title_id: "0100aaaa0000c000".into(),
        version: v,
        display: d.into(),
    };
    game.updates = vec![u(&newer, 262144, "1.1.1"), u(&older, 65536, "1.0.1")];
    assert_eq!(game.update_token(), newer);
    assert!(!has_build(&game) && update_pending(&game));
    let s = card_state(&facts(&game), false, false, false, true);
    assert_eq!(
        (s.action.as_str(), s.status.as_str(), s.action_enabled),
        ("Rebuild", "Update changed", true)
    );
    let a = args::build(&game, "", false, 4);
    assert!(
        a.windows(2)
            .any(|w| w[0] == "--update" && w[1] == native(&newer))
            && !a.contains(&"--no-update".to_string())
    );
    game.built_update = native(&newer);
    assert!(has_build(&game) && !update_pending(&game));
    assert_eq!(
        description(&game, &game.updates[0]),
        "Update [v262144].nsp — 1.1.1 (v262144) — applied"
    );
    game.update_choice = older.clone();
    assert!(update_pending(&game));
    assert_eq!(
        description(&game, &game.updates[1]),
        "Update [v65536].nsp — 1.0.1 (v65536) — selected, build the game to apply it"
    );
    assert_eq!(
        description(&game, &game.updates[0]),
        "Update [v262144].nsp — 1.1.1 (v262144) — in the current build, not selected"
    );
    game.update_choice = "none".into();
    assert!(
        game.active_update().is_none()
            && args::build(&game, "", false, 4).contains(&"--no-update".to_string())
    );
    assert_eq!(token_label(&game, "none"), "the game without updates");
    assert_eq!(token_label(&game, &newer), "update 1.1.1 (v262144)");
    game.update_choice = newer.clone();
    game.updates.clear();
    assert!(game.update_token().is_empty() && update_pending(&game));
    game.built_update.clear();
    assert!(has_build(&game));
    game.updates = vec![u(&newer, 262144, "1.1.1"), u(&older, 65536, "")];
    (game.update_choice, game.built_update) = (older.clone(), newer.clone());
    let loaded = game_json::from_json(&game_json::to_json(&game));
    assert_eq!(
        (
            loaded.updates.len(),
            loaded.updates[0].version,
            loaded.updates[1].label().as_str()
        ),
        (2, 262144, "v65536")
    );
    assert_eq!(
        (loaded.update_choice.as_str(), loaded.built_update.as_str()),
        (older.as_str(), newer.as_str())
    );
    let want = vec![
        "update-info".to_string(),
        "--game".into(),
        native(&game.source),
        native(&newer),
    ];
    assert_eq!(
        args::update_info(&game, "", false, std::slice::from_ref(&newer)),
        want
    );
    write(&format!("{dir}/u.nsz"), b"PFS0");
    write(&format!("{dir}/u.txt"), b"PFS0");
    write(&format!("{dir}/bad.nsp"), b"nope");
    assert!(valid_update(&newer).is_ok());
    assert!(
        valid_update(&format!("{dir}/u.nsz"))
            .unwrap_err()
            .contains("compressed")
    );
    for bad in ["u.txt", "bad.nsp", "gone.nsp"] {
        assert!(valid_update(&format!("{dir}/{bad}")).is_err(), "{bad}");
    }
}

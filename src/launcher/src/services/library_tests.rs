//! The library file (`libraryRoundTripAndCorruption`, a Qt-written golden file, `discoverNspBuild…`,
//! the legacy folders).

use super::build_facts::{facts, has_build};
use super::legacy::migrate;
use super::library_file::{Store, load, save};
use super::library_ops::{add, discover_builds, import_build};
use super::services_tests::{tmp, write};
use super::sources::game_from_source;
use crate::constants::engine as e;
use crate::model::card_state::card_state;
use crate::model::game_json;
use crate::model::library::Library;
use crate::model::path_text::{native, same_path};

#[test]
fn library_round_trip_and_corruption() {
    let (_t, dir) = tmp();
    let file = format!("{dir}/nested/library.json");
    let mut lib = Library::new(&file);
    let mut game = game_from_source(&format!("{dir}/My Game [v1].nup"), "");
    (
        game.cover,
        game.background,
        game.workers,
        game.button_layout,
    ) = ("art.png".into(), "wallpaper.png".into(), 3, "switch".into());
    assert!(add(&mut lib, game.clone()));
    (lib.button_layout, lib.jobs, lib.view_mode, lib.cover_width) = ("xbox".into(), 4, 1, 220);
    (lib.show_background, lib.color_mode, lib.accent_color) =
        (false, "light".into(), "violet".into());
    lib.keys_path = "D:/Keys/prod.keys".into();
    save(&lib, &mut Store::default()).unwrap();
    let mut loaded = Library::new(&file);
    load(&mut loaded, &mut Store::default()).unwrap();
    let g = &loaded.games[0];
    assert_eq!(
        (
            g.source.as_str(),
            g.title.as_str(),
            g.background.as_str(),
            g.workers
        ),
        (game.source.as_str(), "My Game", "wallpaper.png", 3)
    );
    assert_eq!(
        (
            loaded.jobs,
            loaded.view_mode,
            loaded.cover_width,
            loaded.show_background
        ),
        (4, 1, 220, false)
    );
    assert_eq!(
        (loaded.color_mode.as_str(), loaded.accent_color.as_str()),
        ("light", "violet")
    );
    assert_eq!(
        (
            loaded.keys_path.as_str(),
            loaded.button_layout.as_str(),
            g.button_layout.as_str()
        ),
        ("D:/Keys/prod.keys", "xbox", "switch")
    );
    assert_eq!(Library::new(&file).button_layout, "switch");
    assert!(!add(&mut loaded, game));
    std::fs::write(&file, b"bad json").unwrap();
    let (mut broken, mut store) = (Library::new(&file), Store::default());
    assert!(load(&mut broken, &mut store).is_err());
    assert!(save(&broken, &mut store).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"bad json");
}

const QT: &str = "{\n    \"accentColor\": \"mint\",\n    \"askUser\": false,\n    \"buttonLayout\": \"switch\",\n    \"colorMode\": \"dark\",\n    \"coverWidth\": 180,\n    \"dockState\": \"AAAA/wAAAAD9\",\n    \"docked\": true,\n    \"futureKey\": [\n        1,\n        \"two\"\n    ],\n    \"games\": [\n        {\n            \"background\": \"\",\n            \"buildDir\": \"D:/Games/SwitchRecomp Builds/Kart [nxrs]\",\n            \"buildIncomplete\": false,\n            \"builtSource\": \"D:/Games/Kart.nsp\",\n            \"builtUpdate\": \"\",\n            \"buttonLayout\": \"\",\n            \"cover\": \"\",\n            \"dlc\": [\n            ],\n            \"engine\": \"nxrs\",\n            \"gameFuture\": true,\n            \"hintsBuilt\": 0,\n            \"id\": \"0b6f3a52-6b5e-4d55-9a73-3d1c2b0c9e11\",\n            \"lastBuilt\": \"2026-11-19T21:04:55\",\n            \"lastPlayed\": \"\",\n            \"optimization\": 2,\n            \"outputName\": \"Kart\",\n            \"renderer\": \"vulkan\",\n            \"savesOffered\": \"\",\n            \"screenMode\": \"\",\n            \"source\": \"D:/Games/Kart.nsp\",\n            \"title\": \"Kart\",\n            \"updateChoice\": \"\",\n            \"updates\": [\n                {\n                    \"display\": \"1.1.1\",\n                    \"path\": \"D:/Games/Kart [UPD].nsp\",\n                    \"titleId\": \"0100aaaa0000c800\",\n                    \"version\": 262144\n                }\n            ],\n            \"workers\": 0\n        }\n    ],\n    \"geometry\": \"AdnQywADAAA=\",\n    \"jobs\": 8,\n    \"keysPath\": \"\",\n    \"muteOnFocusLoss\": false,\n    \"nxrsBuild\": \"\",\n    \"outputRoot\": \"\",\n    \"pauseOnFocusLoss\": false,\n    \"schema\": 1,\n    \"showBackground\": true,\n    \"toolRoot\": \"\",\n    \"toolRootChosen\": false,\n    \"viewMode\": 0\n}\n";

/// A file as the Qt launcher writes it: read, kept unknown keys and Qt's window blobs, written back in
/// the same format, backed up before the first write.
#[test]
fn qt_written_library_golden_round_trip() {
    let (_t, dir) = tmp();
    let file = format!("{dir}/library.json");
    std::fs::write(&file, QT).unwrap();
    let (mut lib, mut store) = (Library::new(&file), Store::default());
    load(&mut lib, &mut store).unwrap();
    assert_eq!(
        lib.games[0].last_built.map(|d| d.to_string()),
        Some("2026-11-19 21:04:55".into())
    );
    assert_eq!(lib.games[0].updates[0].version, 262144);
    save(&lib, &mut store).unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("{dir}/library.qt-backup.json")).unwrap(),
        QT
    );
    let console = "    \"consolePanel\": {\n        \"floating\": false,\n        \"side\": \"bottom\",\n        \"size\": 190,\n        \"visible\": false\n    },\n";
    let expected = QT.replace(
        "    \"coverWidth\": 180,\n",
        &format!("{console}    \"coverWidth\": 180,\n"),
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
    save(&lib, &mut store).unwrap();
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        2,
        "backed up once"
    );
}

#[test]
fn discover_nsp_build_and_preserve_library() {
    let (_t, dir) = tmp();
    let build = format!("{dir}/MKW NSP build");
    let name = "Mario Kart World 180";
    let source = format!("{dir}/Mario Kart World 1.8.0.nsp");
    write(&source, b"fixture");
    write(
        &format!("{build}/{name}{}", e::EXECUTABLE_SUFFIX),
        b"fixture",
    );
    write(&format!("{build}/{name}{}", e::MODULE_SUFFIX), b"fixture");
    write(
        &format!("{build}/{name}_data/romfs.nsp"),
        format!("SWRNSP1\n{}\n100\n200\n", native(&source)).as_bytes(),
    );
    write(&format!("{build}/{name}_build/gen/nxrs.gen.key"), b"k");
    write(
        &format!("{build}/{name}.ini"),
        b"[Graphics]\nrenderer=vulkan\n",
    );
    let mut library = Library::new(&format!("{dir}/library.json"));
    (library.jobs, library.color_mode) = (3, "light".into());
    let mut old = game_from_source(&format!("{dir}/Older game.nup"), "");
    old.cover = "my cover.png".into();
    assert!(add(&mut library, old.clone()));
    assert_eq!(discover_builds(&mut library, &dir), 1);
    let game = library.games[1].clone();
    assert!(has_build(&game) && game.nxrs());
    assert_eq!(
        (game.format(), game.renderer.as_str()),
        ("DIRECT NSP", "vulkan")
    );
    assert!(same_path(&game.source, &source));
    assert!(card_state(&facts(&game), false, false, false, false).action_enabled);
    assert_eq!(discover_builds(&mut library, &dir), 0);
    assert_eq!(import_build(&mut library, &game), game.id);
    assert_eq!(
        game_json::to_json(&library.games[0]),
        game_json::to_json(&old)
    );
    let mut pending = Library::new(&format!("{dir}/pending.json"));
    let mut unbuilt = game_from_source(&source, "");
    (unbuilt.cover, unbuilt.title, unbuilt.workers) = ("chosen.png".into(), "My MKW".into(), 5);
    add(&mut pending, unbuilt.clone());
    assert_eq!(import_build(&mut pending, &game), unbuilt.id);
    let p = &pending.games[0];
    assert!(has_build(p) && pending.games.len() == 1);
    assert_eq!(
        (p.cover.as_str(), p.title.as_str(), p.workers),
        ("chosen.png", "My MKW", 5)
    );
    std::fs::remove_file(&source).unwrap();
    assert_eq!(
        card_state(&facts(&library.games[1]), false, false, false, true).status,
        "Source offline"
    );
    std::fs::remove_file(game.code_module()).unwrap();
    assert!(!has_build(&library.games[1]));
}

#[test]
fn legacy_data_is_copied_once() {
    let (_t, dir) = tmp();
    let data = format!("{dir}/nxrs");
    write(
        &format!("{dir}/RecompLauncher/SwitchRecompLauncher/library.json"),
        b"older",
    );
    write(
        &format!("{dir}/RecompLauncher/RecompLauncher/library.json"),
        b"newer",
    );
    write(
        &format!("{dir}/RecompLauncher/RecompLauncher/keys/prod.keys"),
        b"k",
    );
    migrate(&data);
    assert_eq!(
        std::fs::read(format!("{data}/library.json")).unwrap(),
        b"newer"
    );
    assert_eq!(
        std::fs::read(format!("{data}/keys/prod.keys")).unwrap(),
        b"k"
    );
    write(
        &format!("{dir}/RecompLauncher/RecompLauncher/library.json"),
        b"changed",
    );
    migrate(&data);
    assert_eq!(
        std::fs::read(format!("{data}/library.json")).unwrap(),
        b"newer"
    );
}

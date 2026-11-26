//! Sources and builds on disk (`importedBuildBringsItsUpdate`, `importChecksAllArtifacts`,
//! `combinedNspBuild`, `nspSourceClassification`, `xciSourceIsAnNxrsGame`, the `[nxrs]` folder).

use super::build_facts::{facts, has_build, playable, update_pending, version};
use super::library_file::{Store, load};
use super::library_ops::{add, import_build};
use super::markers::{adopt_built_update, adopt_package_dlc};
use super::services_tests::{built_game, tmp, write};
use super::sources::{game_from_executable, game_from_source, valid_source};
use crate::constants::engine as e;
use crate::model::args;
use crate::model::build_layout::nxrs_folder_for;
use crate::model::card_state::card_state;
use crate::model::filter::LibraryFilter;
use crate::model::game_json;
use crate::model::library::Library;
use crate::model::path_text::{native, same_path};

#[test]
fn imported_build_brings_its_update() {
    let (_t, dir) = tmp();
    let built = built_game(&dir);
    let (source, update) = (
        format!("{dir}/Game [v0].nsp"),
        format!("{dir}/Game [UPD].nsp"),
    );
    let marker = format!(
        "SWRNSP2\n{}\nkeys k\nromfs 0 10\nupdate {}\npatch 0 10\n",
        native(&source),
        native(&update)
    );
    write(
        &format!("{}/romfs.nsp", built.data_dir()),
        marker.as_bytes(),
    );
    let mut game = game_from_executable(&built.executable()).unwrap();
    assert!(
        same_path(&game.source, &source)
            && game.updates.len() == 1
            && same_path(&game.built_update, &update)
    );
    assert!(has_build(&game) && !update_pending(&game));
    (game.id, game.title) = ("g1".into(), "Game".into());
    let mut entry = game_json::to_json(&game);
    for k in ["updates", "updateChoice", "builtUpdate"] {
        entry.as_object_mut().unwrap().remove(k);
    }
    let file = format!("{dir}/library.json");
    write(
        &file,
        serde_json::json!({"schema": 1, "games": [entry]})
            .to_string()
            .as_bytes(),
    );
    let mut library = Library::new(&file);
    load(&mut library, &mut Store::default()).unwrap();
    assert!(
        library.games[0].updates.len() == 1 && same_path(&library.games[0].built_update, &update)
    );
    assert!(has_build(&library.games[0]));
    assert!(!adopt_built_update(&mut library.games[0].clone()));
}

#[test]
fn import_checks_all_artifacts() {
    let (_t, dir) = tmp();
    let exe = format!("{dir}/My Game{}", e::EXECUTABLE_SUFFIX);
    write(&exe, b"test");
    assert!(game_from_executable(&exe).is_err());
    write(&format!("{dir}/My Game{}", e::MODULE_SUFFIX), b"test");
    std::fs::create_dir_all(format!("{dir}/My Game_data")).unwrap();
    write(&format!("{dir}/My Game_build/gen/nxrs.gen.key"), b"k");
    let mut g = game_from_executable(&exe).unwrap();
    assert!(has_build(&g) && g.nxrs());
    let ready = card_state(&facts(&g), false, false, true, false);
    assert!(ready.action == "Play" && ready.action_enabled && !ready.needs_attention);
    g.build_incomplete = true;
    assert!(!has_build(&g));
    g.build_incomplete = false;
    g.source = format!("{dir}/Missing.nup");
    g.built_source = native(&g.source);
    assert!(has_build(&g));
    // the build carries its data (no in-place marker): it plays with its package gone
    let carried = card_state(&facts(&g), false, false, false, true);
    assert!(carried.action_enabled && carried.status == "Ready to play");
    // a build from before reads its package in place: offline while it is gone
    write(
        &format!("{dir}/My Game_data/{}", e::ROMFS_NUP),
        b"P:/x/Missing.nup\n",
    );
    let offline = card_state(&facts(&g), false, false, false, true);
    assert!(
        !offline.action_enabled && offline.needs_attention && offline.status == "Source offline"
    );
    g.source = "another.nup".into();
    assert!(!has_build(&g));
}

#[test]
fn combined_nsp_build() {
    let (_t, dir) = tmp();
    let build = format!("{dir}/Builds/Kart 1G1U1D");
    let source = format!("{dir}/Kart [v262144] (1G+1U+1D).nsp");
    for p in [
        source.clone(),
        format!("{build}/Kart 1G1U1D{}", e::EXECUTABLE_SUFFIX),
        format!("{build}/Kart 1G1U1D{}", e::MODULE_SUFFIX),
    ] {
        write(&p, b"PFS0fixture");
    }
    write(
        &format!("{build}/Kart 1G1U1D_data/romfs.nsp"),
        format!(
            "SWRNSP2\n{}\nkeys K:/prod.keys\nromfs 800 4000\n",
            native(&source)
        )
        .as_bytes(),
    );
    let game =
        game_from_executable(&format!("{build}/Kart 1G1U1D{}", e::EXECUTABLE_SUFFIX)).unwrap();
    assert!(same_path(&game.source, &source) && has_build(&game) && game.format() == "DIRECT NSP");
    assert_eq!(version(&game), "");
    let mut nacp = vec![0u8; 0x4000];
    nacp[0x3060..0x3065].copy_from_slice(b"1.2.3");
    write(&format!("{build}/Kart 1G1U1D_data/control.nacp"), &nacp);
    assert_eq!(version(&game), "1.2.3");
    let mut fresh = game_from_source(&source, "");
    assert!(
        adopt_package_dlc(&mut fresh, "")
            && fresh.dlc.len() == 1
            && same_path(&fresh.dlc[0], &source)
    );
    assert!(!adopt_package_dlc(&mut fresh, &format!("{dir}/other.nsp")) && fresh.dlc.len() == 1);
    fresh.dlc.clear();
    assert!(!adopt_package_dlc(&mut fresh, &native(&source)) && fresh.dlc.is_empty());
}

#[test]
fn nsp_and_xci_sources() {
    let (_t, dir) = tmp();
    let nsp = format!("{dir}/Game.nsp");
    write(&nsp, &[b"PFS0".as_slice(), &[0u8; 16]].concat());
    valid_source(&nsp).unwrap();
    let game = game_from_source(&nsp, "");
    assert_eq!(game.format(), "DIRECT NSP");
    let filter = LibraryFilter {
        source: 1,
        ..Default::default()
    };
    assert!(filter.matches(&game, &facts(&game), false, false));
    for at in [0x100usize, 0x1100] {
        let path = format!("{dir}/Party [{at}].xci");
        write(
            &path,
            &[vec![0u8; at], b"HEAD".to_vec(), vec![0u8; 0x100]].concat(),
        );
        valid_source(&path).unwrap();
        let g = game_from_source(&path, "");
        assert!(
            g.format() == "DIRECT XCI"
                && g.nxrs()
                && g.build_dir == format!("{dir}/nxrs/Party")
                && g.title == "Party"
        );
    }
    let bad = format!("{dir}/Not a card.xci");
    write(&bad, &[b'x'; 0x200]);
    assert!(valid_source(&bad).unwrap_err().contains("XCI"));
}

/// Every new game builds into `<source folder>\nxrs\<name>` (or `<Build location>\nxrs\<name>`) and
/// plays from there; an entry saved without an engine loads as nxrs, moved to such a folder, and needs
/// a build.
#[test]
fn games_build_into_their_nxrs_folder() {
    let (_t, dir) = tmp();
    let source = format!("{dir}/Kart [v0].nsp");
    write(&source, b"PFS0");
    assert_eq!(
        game_from_source(&source, "E:/Builds/").build_dir,
        "E:/Builds/nxrs/Kart"
    );
    let mut game = game_from_source(&source, "");
    assert!(game.nxrs() && game.build_dir == format!("{dir}/nxrs/Kart"));
    let nxrs_dir = game.build_dir.clone();
    let mut old = game_json::to_json(&game);
    old["engine"] = serde_json::Value::Null;
    old["buildDir"] = serde_json::json!(format!("{dir}/{}/Kart", e::LEGACY_BUILDS));
    let moved = game_json::from_json(&old);
    assert!(moved.nxrs() && moved.build_dir == nxrs_dir && !has_build(&moved));
    let s = card_state(&facts(&moved), false, false, false, true);
    assert!(s.action == "Build" && s.status == "Needs build");
    assert_eq!(nxrs_folder_for(&format!("{nxrs_dir}/")), nxrs_dir);
    for p in [game.executable(), game.code_module()] {
        write(&p, b"x");
    }
    // a build that carries its data: its record names the package
    write(
        &format!("{}/{}", game.data_dir(), e::ROMFS_SOURCE),
        format!("SWRNSP2\n{}\nkeys k\nmarker romfs.nsp\n", native(&source)).as_bytes(),
    );
    std::fs::create_dir_all(format!("{}/romfs", game.data_dir())).unwrap();
    game.built_source = game.source.clone();
    assert!(playable(&game), "the nxrs folder's build plays");
    let imported = game_from_executable(&game.executable()).unwrap();
    let mut library = Library::new(&format!("{dir}/library.json"));
    let mut unbuilt = game_from_source(&source, "");
    unbuilt.build_dir = format!("{dir}/elsewhere/Kart");
    assert!(add(&mut library, unbuilt.clone()));
    assert_eq!(import_build(&mut library, &imported), unbuilt.id);
    assert!(playable(&library.games[0]));
    game.workers = 6;
    let keys = format!("{dir}/prod.keys");
    let a = args::build(&game, &keys, false, 8);
    let head = [
        "build",
        &native(&source),
        "--outdir",
        &native(&game.build_dir),
        "-o",
        "Kart",
        "--jobs",
        "6",
        "--machine",
    ]
    .map(String::from);
    assert_eq!(&a[..9], &head);
    assert!(!a.contains(&"--keys".to_string()));
    assert!(
        args::build(&game, &keys, true, 8)
            .windows(2)
            .any(|w| w[0] == "--keys" && w[1] == native(&keys))
    );
}

//! The model's rules: paths, JSON, filters, statuses, names, the engine's lines (`tests.cpp`).

use super::card_state::{card_state, status_for};
use super::doctor;
use super::facts::Facts;
use super::filter::LibraryFilter;
use super::game::Game;
use super::game_json;
use super::names::{build_folder_for, output_name_for, title_from_name};
use super::output_lines::{UpdateLine, progress, reports_package_dlc, update_line};
use super::path_text::{clean, native, same_path, suffix};
use super::play::{
    effective_button_layout, effective_docked, normalize_button_layout, normalize_screen_mode,
    valid_button_layout,
};
use super::qtjson;
use serde_json::json;

#[test]
fn path_formatting_does_not_invalidate_build() {
    #[cfg(windows)]
    {
        assert!(same_path("P:\\Games\\Title.nup", "P:/Games/Title.nup"));
        assert_eq!(clean("D:\\x\\..\\y\\"), "D:/y");
        assert_eq!(clean("D:/"), "D:/");
        assert_eq!(super::path_text::parent("D:/a"), "D:/");
        assert_eq!(native("D:/a b/c"), "D:\\a b\\c");
    }
    assert!(same_path("", "") && !same_path("", "x"));
    assert_eq!(clean("a/b/../c//d/."), "a/c/d");
    assert_eq!(suffix("x/a.NSP"), "nsp");
}

#[test]
fn qt_json_writer() {
    let v: serde_json::Value =
        serde_json::from_str(r#"{"b":[],"a":{"x":1,"y":[1,"z"]},"s":"D:\\a \"q\""}"#).unwrap();
    let want = "{\n    \"a\": {\n        \"x\": 1,\n        \"y\": [\n            1,\n            \"z\"\n        ]\n    },\n    \"b\": [\n    ],\n    \"s\": \"D:\\\\a \\\"q\\\"\"\n}\n";
    assert_eq!(qtjson::to_string(&v), want);
}

#[test]
fn search_and_filters_combine() {
    let game = Game {
        title: "Donkey Kong Bananza".into(),
        source: "P:/Bananza.nup".into(),
        ..Game::default()
    };
    let f = Facts {
        in_place: true,
        ..Facts::default()
    };
    let mut filter = LibraryFilter {
        text: "BANAN".into(),
        state: 2,
        source: 1,
    };
    assert!(filter.matches(&game, &f, false, false));
    assert_eq!(filter.active_count(), 3);
    filter.source = 2;
    assert!(!filter.matches(&game, &f, false, false));
    filter.source = 1;
    filter.state = 3;
    assert!(!filter.matches(&game, &f, false, false));
    assert!(filter.matches(&game, &f, true, false));
    filter.state = 2;
    assert!(!filter.matches(&game, &f, false, true));
    filter.state = 1;
    assert!(!filter.matches(&game, &f, false, false));
    filter = LibraryFilter::default();
    assert!(filter.matches(&game, &f, false, false));
    assert_eq!(filter.active_count(), 0);
}

#[test]
fn per_card_action_targets_correct_operation() {
    let f = Facts {
        source_available: true,
        in_place: true,
        ..Facts::default()
    };
    let s = card_state(&f, false, false, false, true);
    assert_eq!(s.action, "Build");
    assert!(s.needs_attention && s.action_enabled);
    assert!(!card_state(&f, false, false, true, true).action_enabled);
    let s = card_state(&f, true, false, true, true);
    assert!(s.action == "Stop" && s.action_enabled);
    let s = card_state(&f, false, true, true, true);
    assert!(s.action == "Cancel build" && s.action_enabled);
    // one "Source offline" rule for the card and the list: any in-place package
    let offline = Facts {
        has_build: true,
        in_place: true,
        ..Facts::default()
    };
    let s = card_state(&offline, false, false, false, true);
    assert!(s.status == "Source offline" && !s.action_enabled && s.needs_attention);
    assert_eq!(status_for(&offline, false, false, false), "Source offline");
    let turbo = Facts {
        turbo: true,
        has_build: true,
        source_available: true,
        ..Facts::default()
    };
    let s = card_state(&turbo, false, false, false, true);
    assert_eq!(
        (s.action.as_str(), s.status.as_str()),
        ("Build", "Needs nxrs build")
    );
    assert_eq!(status_for(&turbo, false, false, false), "Needs nxrs build");
    let pending = Facts {
        update_pending: true,
        source_available: true,
        ..Facts::default()
    };
    assert_eq!(
        card_state(&pending, false, false, false, true).action,
        "Rebuild"
    );
    assert_eq!(
        status_for(&pending, false, false, false),
        "Update changed — build again"
    );
}

#[test]
fn layouts_and_screen_modes() {
    let mut game = Game::default();
    assert_eq!(effective_button_layout(&game, "xbox"), "xbox");
    assert_eq!(effective_button_layout(&game, "bogus"), "switch");
    game.button_layout = "switch".into();
    assert_eq!(effective_button_layout(&game, "xbox"), "switch");
    assert!(
        valid_button_layout("") && valid_button_layout("xbox") && !valid_button_layout("label")
    );
    assert_eq!(normalize_button_layout("nintendo"), "switch");
    assert_eq!(normalize_button_layout("labels"), "xbox");
    assert_eq!(
        game_json::from_json(&json!({"buttonLayout": "nintendo"})).button_layout,
        "switch"
    );
    assert!(
        game_json::from_json(&json!({"buttonLayout": "sega"}))
            .button_layout
            .is_empty()
    );
    assert!(effective_docked(&game, true) && !effective_docked(&game, false));
    game.screen_mode = "handheld".into();
    assert!(!effective_docked(&game, true));
    assert_eq!(normalize_screen_mode("Docked"), "tv");
    assert_eq!(normalize_screen_mode("x"), "");
    assert_eq!(
        game_json::from_json(&json!({"id": "x", "screenMode": "sideways"})).screen_mode,
        ""
    );
}

#[test]
fn names_and_folders() {
    assert_eq!(
        output_name_for("Pok\u{e9}mon Legends - Arceus (1G+1U)"),
        "Pokemon Legends - Arceus 1G1U"
    );
    assert_eq!(output_name_for("\u{30bc}\u{30eb}\u{30c0}"), "Game");
    assert_eq!(title_from_name("My Game [v1]"), "My Game");
    assert_eq!(title_from_name("[x]"), "Untitled game");
    assert_eq!(build_folder_for("D:/B/Kart/", true), "D:/B/Kart [nxrs]");
    assert_eq!(build_folder_for("D:/B/Kart [nxrs]", false), "D:/B/Kart");
}

#[test]
fn engine_lines() {
    assert_eq!(
        progress("@progress\tcompile\t3\t9"),
        Some(("Compiling native code".into(), 3, 9))
    );
    assert_eq!(progress("@result\tok\tx"), None);
    assert_eq!(
        progress("C generated: 4/10 chunks"),
        Some(("Generating native C".into(), 4, 10))
    );
    assert!(reports_package_dlc(
        "DLC: this package also holds add-on content 0100152000023001; install it"
    ));
    assert!(!reports_package_dlc(
        "DLC: installed 0100152000023001 v65536"
    ));
    let line = format!(
        "UPDATE\t{}\t01001f5010dfa000\t262144\t1.1.1",
        native("D:/Games/Update [v262144].nsp")
    );
    match update_line(&line) {
        UpdateLine::Found(u) => assert_eq!(
            (u.path.as_str(), u.version, u.display.as_str()),
            ("D:/Games/Update [v262144].nsp", 262144, "1.1.1")
        ),
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(update_line("UPDATE\tD:\\bare.nca\t01001f5010dfa000\t0\t"), UpdateLine::Found(u) if u.display.is_empty())
    );
    assert!(
        matches!(update_line("UPDATE error: X.nsp: this update is for 0100152000022000"), UpdateLine::Error(e) if e.starts_with("X.nsp"))
    );
    assert_eq!(update_line("anything else"), UpdateLine::Other);
}

#[test]
fn nxrs_doctor_checks() {
    let d = doctor::parse(
        "ok   CPU: AVX2\r\n     toolchain: llvm-mingw\r\nFAIL Vulkan: no device\r\nok   free disk (D:\\x): 10.0 GB\r\n",
        1,
    );
    assert_eq!((d.checks.len(), d.failed()), (3, 1));
    assert!(!d.ok());
    assert_eq!(d.notes, vec!["toolchain: llvm-mingw".to_string()]);
    assert_eq!(d.checks[1].text, "Vulkan: no device");
    assert!(d.text().contains("\u{2717} Vulkan: no device"));
    assert!(doctor::parse("ok   one\n", 0).ok());
    assert!(
        !doctor::parse("nxrs-build: unknown command\n", 2)
            .error
            .is_empty()
    );
}

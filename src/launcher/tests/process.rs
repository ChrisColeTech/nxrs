//! Engine runs through the fake engine, driven through the launcher's own flows
//! (`dlcInstallerRunsTheEngineSync`, `updateProbeAsksTheEngine`, `nxrsEngineAnswersUpdatesAndDlc`,
//! `nxrsDoctorChecks`, `matchesRunningExecutableByFullPath`).

mod common;

use common::*;
use nxrs_launcher::flows::{DlcReason, Outcome};
use nxrs_launcher::model::game::Game;
use nxrs_launcher::model::path_text::{from_native, parent, same_path};
use nxrs_launcher::services::{dlc_files, platform, run, toolchain::Toolchain};

#[test]
fn dlc_sync_runs_the_engine() {
    let (_t, dir) = tmp();
    let mut game = built_game(&dir);
    let (good, refused) = (
        format!("{dir}/Booster pack.nsp"),
        format!("{dir}/refused.nsp"),
    );
    game.dlc = vec![good.clone(), refused.clone()];
    let mut app = app_with(&dir, vec![game.clone()]);
    std::fs::remove_file(&app.tools.keys).unwrap();
    assert!(
        !app.start_dlc_sync("g", DlcReason::Installed),
        "no keys: said before anything starts"
    );
    write(&app.tools.keys, b"header_key = 00");
    assert!(app.start_dlc_sync("g", DlcReason::Installed));
    assert!(
        !app.start_dlc_sync("g", DlcReason::Installed),
        "one at a time"
    );
    settle(&mut app);
    let titles = dlc_files::installed(&game.data_dir());
    assert!(titles.len() == 1 && same_path(&titles[0].source, &good));
    // the data folder is the truth: the refused package left the list
    assert_eq!(app.lib.games[0].dlc.len(), 1);
    assert!(
        app.console
            .lines
            .iter()
            .any(|l| l == "DLC: installed 0100152000023001 v65536")
    );
    assert!(dlc_files::up_to_date(&app.lib.games[0]));
    // a game that is not built has nothing to install into
    let unbuilt = Game {
        id: "u".into(),
        title: "U".into(),
        build_dir: format!("{dir}/nowhere"),
        output_name: "X".into(),
        engine: "nxrs".into(),
        dlc: vec![good],
        ..Game::default()
    };
    app.lib.games.push(unbuilt);
    assert!(!app.start_dlc_sync("u", DlcReason::Installed));
}

#[test]
fn install_dlc_from_the_command_line_says_the_outcome() {
    let (_t, dir) = tmp();
    let game = built_game(&dir);
    let package = format!("{dir}/Booster.nsp");
    write(&package, b"PFS0....");
    let mut app = app_with(&dir, vec![game]);
    assert!(app.install_dlc_packages(std::slice::from_ref(&package), false));
    settle(&mut app);
    match app.outcomes.first() {
        Some(Outcome::Dlc(true, text)) => assert_eq!(
            text,
            "Installed: 0100152000023001 v65536. It is in effect the next time you play."
        ),
        other => panic!("{other:?}"),
    }
    app.outcomes.clear();
    app.remove_dlc(&[package]);
    settle(&mut app);
    assert!(
        matches!(app.outcomes.first(), Some(Outcome::Dlc(true, t)) if t == "No DLC is installed now."),
        "{:?}",
        app.outcomes
    );
}

#[test]
fn update_probe_asks_the_engine() {
    let (_t, dir) = tmp();
    let game = built_game(&dir);
    let (update, other) = (format!("{dir}/Update.nsp"), format!("{dir}/other game.nsp"));
    write(&update, b"PFS0....");
    write(&other, b"PFS0....");
    let mut app = app_with(&dir, vec![game]);
    assert!(app.install_update_packages(&[update.clone(), other], false));
    assert!(
        app.probe_busy(),
        "one check at a time: the next waits for this one"
    );
    settle(&mut app);
    let g = &app.lib.games[0];
    assert!(g.updates.len() == 1 && same_path(&g.updates[0].path, &update));
    assert_eq!(
        (g.updates[0].version, g.updates[0].display.as_str()),
        (262144, "1.1.1")
    );
    match app.outcomes.first() {
        Some(Outcome::Update(false, text)) => {
            assert!(text.starts_with("other game.nsp: this update is for another game\n\nAdded: 1.1.1 (v262144). Build the game again"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    std::fs::remove_file(&app.lib.games[0].source).unwrap();
    app.outcomes.clear();
    let again = format!("{dir}/Again.nsp");
    write(&again, b"PFS0....");
    assert!(!app.install_update_packages(&[again], false));
    assert!(
        matches!(app.outcomes.first(), Some(Outcome::Update(false, t)) if t.contains("own package is missing"))
    );
}

#[test]
fn nxrs_doctor_runs_the_engine() {
    let none = run::doctor(&Toolchain::default());
    assert!(!none.error.is_empty());
    let tools = Toolchain {
        nxrs_build: from_native(FAKE),
        nxrs_contract: 2,
        ..Toolchain::default()
    };
    let d = run::doctor(&tools);
    assert!(d.error.is_empty(), "{}", d.error);
    assert_eq!((d.exit_code, d.checks.len(), d.failed()), (1, 4, 1));
    assert!(
        d.checks[2].text.starts_with("Vulkan device: Old GPU")
            && d.checks[3].text.contains("120.5 GB")
    );
    // the real engine, when the developer names it (NXRS_DOCTOR_EXE): it answers in the same format
    if let Ok(real) = std::env::var("NXRS_DOCTOR_EXE") {
        let r = run::doctor(&Toolchain {
            nxrs_build: real,
            nxrs_contract: 2,
            ..Toolchain::default()
        });
        assert!(r.error.is_empty() && r.checks.len() >= 5, "{}", r.text());
    }
}

#[test]
fn finds_the_engine_and_its_contract() {
    let (_t, dir) = tmp();
    let engine = format!("{dir}/engine dev/target/release");
    std::fs::create_dir_all(&engine).unwrap();
    let exe = format!("{engine}/nxrs-build{}", std::env::consts::EXE_SUFFIX);
    std::fs::copy(FAKE, &exe).unwrap();
    let t = Toolchain::find(&engine, "");
    assert!(
        same_path(&t.nxrs_build, &exe) && t.nxrs_contract == 2 && t.can_build(),
        "{}",
        t.problem()
    );
    write(
        &format!("{engine}/engine.json"),
        br#"{"contract": 1, "engine": "nxrs", "version": "0.0.1"}"#,
    );
    let t = Toolchain::find(&exe, "");
    assert_eq!((t.nxrs_contract, t.nxrs_version.as_str()), (1, "0.0.1"));
    assert!(t.problem().contains("contract 1"));
    assert!(
        Toolchain::find(&format!("{dir}/nowhere"), "")
            .nxrs_build
            .is_empty()
            || std::env::var("NXRS_BUILD").is_ok()
    );
}

#[test]
fn matches_running_executable_by_full_path() {
    let me = std::env::current_exe().unwrap();
    let path = from_native(&me.to_string_lossy());
    let name = me.file_stem().unwrap().to_string_lossy().into_owned();
    let mine = Game {
        id: "self".into(),
        output_name: name.clone(),
        build_dir: parent(&path),
        ..Game::default()
    };
    let (_t, dir) = tmp();
    let other = Game {
        id: "other".into(),
        output_name: name,
        build_dir: dir,
        ..Game::default()
    };
    let running = platform::find_running_games(&[mine, other.clone()]);
    assert_eq!(running.get("self"), Some(&std::process::id()));
    assert!(!running.contains_key("other"));
    assert!(
        !platform::request_close(std::process::id(), &other.executable()),
        "another path: nothing is asked"
    );
}

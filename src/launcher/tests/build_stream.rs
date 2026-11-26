//! A build through the fake engine (its own test binary: the fake's behaviour is set by environment
//! variables): 10,000 lines stream and the build finishes; a cancel ends the build and the process it
//! started.

mod common;

use common::*;
use nxrs_launcher::flows::Outcome;
use nxrs_launcher::model::game::Game;
use nxrs_launcher::model::path_text::{from_native, parent};
use nxrs_launcher::services::{build_facts, platform};
use std::time::{Duration, Instant};

fn unbuilt(dir: &str) -> Game {
    let source = format!("{dir}/games/Kart.nsp");
    write(&source, b"PFS0................");
    Game {
        id: "k".into(),
        title: "Kart".into(),
        output_name: "Kart".into(),
        engine: "nxrs".into(),
        build_dir: format!("{dir}/builds/Kart [nxrs]"),
        source,
        ..Game::default()
    }
}

/// How many fake engines run (listed, never killed, by image name).
fn fake_processes() -> usize {
    let path = from_native(FAKE);
    let name = std::path::Path::new(FAKE)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let probe = Game {
        id: "fake".into(),
        output_name: name.clone(),
        build_dir: parent(&path),
        ..Game::default()
    };
    if !cfg!(windows) {
        return platform::find_running_games(&[probe]).len();
    }
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}.exe"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.to_lowercase().starts_with(&name))
        .count()
}

#[test]
fn builds_stream_and_cancel_kills_the_tree() {
    // SAFETY: this test binary has one test; nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("FAKE_LINES", "10000") };
    let (_t, dir) = tmp();
    let mut app = app_with(&dir, vec![unbuilt(&dir)]);
    app.start_build();
    assert!(app.lib.games[0].build_incomplete);
    let start = Instant::now();
    settle(&mut app);
    assert!(start.elapsed() < Duration::from_secs(20));
    assert_eq!(app.outcomes, vec![Outcome::Smoke(true)]);
    let g = &app.lib.games[0];
    assert!(build_facts::has_build(g) && !g.build_incomplete && g.last_built.is_some());
    assert_eq!(app.console.title, "Build output — complete");
    assert!(app.console.lines.iter().any(|l| l == "compile unit 9999"));
    assert!(
        !app.console.lines.iter().any(|l| l.starts_with('@')),
        "machine lines are not shown"
    );
    let log = read(&g.build_log());
    assert!(log.starts_with("nxrs build · ") && log.contains("@progress\tcompile\t10000\t10000"));
    assert!(
        log.trim_end()
            .ends_with("Build complete. Your game is ready to play.")
    );
    assert!(
        read(&g.ini_path()).contains("button_layout=switch"),
        "the game's settings were written"
    );

    // cancel: the build and the process it started end
    unsafe {
        std::env::set_var("FAKE_LINES", "100000");
        std::env::set_var("FAKE_SLEEP_MS", "20");
        std::env::set_var("FAKE_CHILD", "1");
    }
    let (_t2, dir2) = tmp();
    let mut app = app_with(&dir2, vec![unbuilt(&dir2)]);
    app.start_build();
    // wait for the state, not a fixed sleep: a loaded CI runner starts the fake and its child late
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut running = 0;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        app.tick();
        running = fake_processes();
        if app.progress.total > 0 && running >= 2 {
            break;
        }
    }
    assert!(
        app.progress.total > 0,
        "progress arrived: total {}",
        app.progress.total
    );
    assert!(
        running >= 2,
        "the build and its child run: {running} fake processes"
    );
    app.stop();
    settle(&mut app);
    assert_eq!(app.outcomes, vec![Outcome::Smoke(false)]);
    assert!(
        app.console
            .lines
            .iter()
            .any(|l| l.contains("Build cancelled."))
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fake_processes(), 0, "nothing of the build is left");
}

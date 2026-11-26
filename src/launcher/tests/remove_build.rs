//! Remove from library with "Also delete the build" (owner, 2026-11-26): ticked, the player's files are
//! kept and the build folder deleted, and the next build of the game puts them back; unticked, nothing
//! on disk changes; a running game is refused. The package is never touched.

mod common;

use common::*;
use nxrs_launcher::flows::surface::Surface;
use nxrs_launcher::flows::{Action, App, Outcome};
use nxrs_launcher::model::game::Game;
use std::path::Path;

fn kart(dir: &str) -> Game {
    let source = format!("{dir}/games/Kart.nsp");
    write(&source, b"PFS0................");
    Game {
        id: "k".into(),
        title: "Kart".into(),
        output_name: "Kart".into(),
        build_dir: format!("{dir}/games/nxrs/Kart"),
        source,
        ..Game::default()
    }
}

/// A build of Kart through the fake engine, with saves, an SD card file, a cheat and a mod in it.
fn built_with_saves(dir: &str) -> App {
    let mut app = app_with(dir, vec![kart(dir)]);
    app.start_build();
    settle(&mut app);
    assert_eq!(app.outcomes, vec![Outcome::Smoke(true)]);
    let g = app.lib.games[0].clone();
    write(&format!("{}/save/0000/slot.bin", g.data_dir()), b"progress");
    write(&format!("{}/sdmc/photo.jpg", g.data_dir()), b"jpg");
    write(&format!("{}/cheats/ABCD.txt", g.data_dir()), b"[x]");
    write(&format!("{}/mods/Fast/patch.ips", g.build_dir), b"PATCH");
    app.outcomes.clear();
    app
}

#[test]
fn ticked_keeps_saves_deletes_the_build_and_a_rebuild_restores_them() {
    let (_t, dir) = tmp();
    let mut app = built_with_saves(&dir);
    let g = app.lib.games[0].clone();
    // the question offers the deletion, ticked, with the build's size
    app.remove_game();
    let Some(Surface::Question { check: Some(c), .. }) = app.surfaces.last() else {
        panic!("no question with a tick box");
    };
    assert!(c.on && c.label.starts_with("Also delete the build (") && c.label.ends_with("MB)"));
    assert_eq!(c.heading, "Kart");
    assert_eq!(c.yes_on, Action::ConfirmRemove("k".into(), true));
    app.surfaces.clear();
    app.confirm_remove("k", true);
    assert!(app.lib.games.is_empty() && !Path::new(&g.build_dir).exists());
    assert!(
        !Path::new(&format!("{dir}/games/nxrs")).exists(),
        "the empty nxrs folder goes"
    );
    assert!(
        Path::new(&g.source).is_file(),
        "the package is never touched"
    );
    // no npdm in the fake build: the kept folder is named after the game
    let kept = format!("{dir}/saves/Kart");
    assert_eq!(read(&format!("{kept}/data/save/0000/slot.bin")), "progress");
    assert_eq!(read(&format!("{kept}/mods/Fast/patch.ips")), "PATCH");
    // added and built again: the saves go back, the kept folder goes
    app.lib.games = vec![kart(&dir)];
    app.selected = "k".into();
    app.refresh_facts();
    app.start_build();
    settle(&mut app);
    assert_eq!(app.outcomes, vec![Outcome::Smoke(true)]);
    let g = &app.lib.games[0];
    assert_eq!(
        read(&format!("{}/save/0000/slot.bin", g.data_dir())),
        "progress"
    );
    assert_eq!(read(&format!("{}/sdmc/photo.jpg", g.data_dir())), "jpg");
    assert_eq!(read(&format!("{}/cheats/ABCD.txt", g.data_dir())), "[x]");
    assert_eq!(
        read(&format!("{}/mods/Fast/patch.ips", g.build_dir)),
        "PATCH"
    );
    assert!(!Path::new(&kept).exists());
    assert!(
        app.console
            .lines
            .iter()
            .any(|l| l.starts_with("Kept saves put back"))
    );
}

#[test]
fn unticked_leaves_every_file() {
    let (_t, dir) = tmp();
    let mut app = built_with_saves(&dir);
    let g = app.lib.games[0].clone();
    app.confirm_remove("k", false);
    assert!(app.lib.games.is_empty());
    assert!(Path::new(&g.executable()).is_file());
    assert_eq!(
        read(&format!("{}/save/0000/slot.bin", g.data_dir())),
        "progress"
    );
    assert!(!Path::new(&format!("{dir}/saves")).exists());
}

#[test]
fn a_running_game_is_refused() {
    let (_t, dir) = tmp();
    let mut app = built_with_saves(&dir);
    let g = app.lib.games[0].clone();
    app.running.insert("k".into(), 1);
    app.confirm_remove("k", true);
    assert_eq!(app.lib.games.len(), 1, "the entry stays");
    assert!(Path::new(&g.executable()).is_file());
    assert_eq!(
        read(&format!("{}/save/0000/slot.bin", g.data_dir())),
        "progress"
    );
    assert!(!Path::new(&format!("{dir}/saves")).exists());
}

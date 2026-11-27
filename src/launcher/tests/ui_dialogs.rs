//! Through the real shell: Remove from library's tick box (ticked by default; unticked, Remove keeps the
//! build) in the question box, and Settings > Build defaults' Move, the only way besides
//! `--migrate-builds` that old-layout build folders move (opening the library never moves them).

mod common;

use common::*;
use egui::accesskit::{Role, Toggled};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use nxrs_launcher::flows::{Action, App};
use nxrs_launcher::model::game::Game;
use nxrs_launcher::services::jobs::noop;
use nxrs_launcher::ui::shell::Shell;
use std::path::Path;

fn harness(app: App) -> Harness<'static, Shell> {
    HarnessBuilder::default()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, s: &mut Shell| s.frame(ui), Shell::new(app, noop()))
}

/// Remove from library on a built game; the tick box as found.
fn remove_dialog(dir: &str) -> (Harness<'static, Shell>, Game) {
    let g = built_game(dir);
    let mut app = app_with(dir, vec![g.clone()]);
    app.dispatch(Action::RemoveGame);
    let mut h = harness(app);
    h.run_steps(4);
    (h, g)
}

const TICK: &str = "Also delete the build (1 MB)";

#[test]
fn remove_ticked_by_default_deletes_the_build() {
    let (_t, dir) = tmp();
    let (mut h, g) = remove_dialog(&dir);
    let tick = h.get_by_role_and_label(Role::CheckBox, TICK);
    assert_eq!(tick.accesskit_node().toggled(), Some(Toggled::True));
    // the question is the heading; its one button is the verb (no Yes / No)
    assert!(
        h.query_by_label("Remove “Kart” from the library?")
            .is_some()
    );
    assert!(h.query_by_label("Yes").is_none() && h.query_by_label("No").is_none());
    h.get_by_label("Remove").click();
    h.run_steps(4);
    assert!(h.state().app.lib.games.is_empty());
    assert!(!Path::new(&g.build_dir).exists(), "the build went");
    assert!(
        Path::new(&g.source).is_file(),
        "the package is never touched"
    );
}

#[test]
fn remove_unticked_keeps_the_build() {
    let (_t, dir) = tmp();
    let (mut h, g) = remove_dialog(&dir);
    h.get_by_role_and_label(Role::CheckBox, TICK).click();
    h.run_steps(2);
    let tick = h.get_by_role_and_label(Role::CheckBox, TICK);
    assert_eq!(tick.accesskit_node().toggled(), Some(Toggled::False));
    h.get_by_label("Remove").click();
    h.run_steps(4);
    assert!(h.state().app.lib.games.is_empty());
    assert!(Path::new(&g.executable()).is_file(), "the build stays");
}

#[test]
fn old_layout_builds_move_only_from_settings() {
    let (_t, dir) = tmp();
    let old = format!("{dir}/SwitchRecomp Builds/Kart [nxrs]");
    write(&format!("{old}/Kart.exe"), b"exe");
    let lib = format!("{dir}/library.json");
    let entry = serde_json::json!({"id": "g", "title": "Kart", "engine": "nxrs",
        "outputName": "Kart", "source": format!("{dir}/Kart.nsp"), "buildDir": old});
    let file = serde_json::json!({"schema": 1, "games": [entry]});
    write(&lib, file.to_string().as_bytes());
    // opening the library moves nothing
    let mut app = App::new(&lib, false, noop());
    app.open();
    assert_eq!(app.lib.games[0].build_dir, old);
    assert!(Path::new(&format!("{old}/Kart.exe")).is_file());
    app.dispatch(Action::Settings(1));
    let mut h = harness(app);
    h.run_steps(4);
    assert!(
        h.query_by_label_contains("Builds in the old layout: 1.")
            .is_some()
    );
    h.get_by_label("Move").click();
    h.run_steps(4);
    let moved = format!("{dir}/nxrs/Kart");
    assert_eq!(h.state().app.lib.games[0].build_dir, moved);
    assert!(Path::new(&format!("{moved}/Kart.exe")).is_file());
    assert!(
        read(&lib).contains("nxrs/Kart"),
        "saved at once, not on Apply"
    );
    assert!(h.query_by_label_contains("Moved: Kart:").is_some());
}

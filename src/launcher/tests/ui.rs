//! The window by its accessible names (`gui_tests.cpp`): the Game menu, the filters popover and Escape,
//! Properties > DLC, the DLC and Updates windows (drawn inside the main window, as without a second
//! window the harness has), the palette that the app glyph follows.

mod common;

use common::*;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, HarnessBuilder};
use nxrs_launcher::flows::surface::Surface;
use nxrs_launcher::flows::view_state::Filters;
use nxrs_launcher::flows::{Action, App};
use nxrs_launcher::model::game_update::GameUpdate;
use nxrs_launcher::services::jobs::noop;
use nxrs_launcher::ui::chrome::menu_model::{self, Item};
use nxrs_launcher::ui::shell::Shell;
use nxrs_launcher::ui::theme;

fn harness(app: App) -> Harness<'static, Shell> {
    let shell = Shell::new(app, noop());
    HarnessBuilder::default()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, s: &mut Shell| s.frame(ui), shell)
}

fn labels(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .map(|i| {
            if let Item::Command { label, .. } = i {
                label.clone()
            } else {
                "-".into()
            }
        })
        .collect()
}

#[test]
fn game_menu_installs_dlc() {
    let (_t, dir) = tmp();
    let mut app = App::new(&format!("{dir}/library.json"), true, noop());
    app.open();
    let menu = menu_model::menu(&app, "Game");
    let want = [
        "Play",
        "Stop",
        "Build",
        "-",
        "Install DLC…",
        "Install Updates…",
        "Copy saves from the Turbo build…",
        "-",
        "Remove from library…",
    ];
    assert_eq!(labels(&menu), want);
    let enabled = |label: &str| {
        menu.iter()
            .any(|i| matches!(i, Item::Command { label: l, enabled: true, .. } if l == label))
    };
    assert!(
        !enabled("Copy saves from the Turbo build…"),
        "no Turbo build, no saves"
    );
    assert!(
        !enabled("Install DLC…"),
        "the preview library has nothing to install into"
    );
    let mut h = harness(app);
    h.run();
    h.get_by_label("Game").click();
    h.run();
    assert!(
        h.query_by_label("Install DLC…").is_some(),
        "the menu opened"
    );
    assert!(
        !std::path::Path::new(&format!("{dir}/library.json")).exists(),
        "the demo saves nothing"
    );
}

#[test]
fn filters_clear_as_one_change_and_escape_closes() {
    let mut f = Filters {
        search: "Bananza".into(),
        state: 2,
        source: 1,
        sort: Some(2),
        ..Filters::default()
    };
    f.clear();
    assert_eq!(
        (f.search.as_str(), f.state, f.source, f.sort),
        ("", 0, 0, Some(0))
    );
    let (_t, dir) = tmp();
    let app = app_with(&dir, vec![built_game(&dir)]);
    let mut h = harness(app);
    h.run();
    h.get_by_label("Search & Filter").click();
    h.run();
    assert!(h.query_by_label("Search game titles").is_some());
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(!h.state().app.filters.open);
}

#[test]
fn properties_list_and_remove_dlc() {
    let (_t, dir) = tmp();
    let mut game = built_game(&dir);
    let (booster, later) = (
        format!("{dir}/Booster [v65536].nsp"),
        format!("{dir}/Later.nsz"),
    );
    write(&booster, b"PFS0");
    write(&later, b"PFS0");
    write(
        &format!("{}/dlc.manifest", game.data_dir()),
        format!("0100152000023001 65536 data {booster}\n").as_bytes(),
    );
    game.dlc = vec![booster.clone(), later.clone()];
    let mut app = app_with(&dir, vec![game]);
    app.dispatch(Action::Properties(4));
    let mut h = harness(app);
    h.run();
    assert!(
        h.query_by_label("Booster [v65536].nsp — 0100152000023001 v65536")
            .is_some()
    );
    h.get_by_label("Later.nsz — not installed yet").click();
    h.run();
    h.key_press(egui::Key::Delete);
    h.run();
    let Some(Surface::Properties(d)) = h.state().app.surfaces.last() else {
        panic!("Properties is open")
    };
    assert_eq!(d.dlc, vec![booster], "removed from the draft only");
    assert_eq!(
        h.state().app.lib.games[0].dlc.len(),
        2,
        "nothing changes before Apply"
    );
    h.get_by_label("Cancel").click();
    h.run();
    assert!(h.state().app.surfaces.is_empty());
}

#[test]
fn dlc_window_removes_several() {
    let (_t, dir) = tmp();
    let mut game = built_game(&dir);
    let names = ["First.nsp", "Second.nsz", "Third.xci"];
    let paths: Vec<String> = names.iter().map(|n| format!("{dir}/{n}")).collect();
    for p in &paths {
        write(p, b"PFS0");
    }
    let manifest: String = paths
        .iter()
        .enumerate()
        .map(|(i, p)| format!("010015200002300{} 1 data {p}\n", i + 1))
        .collect();
    write(
        &format!("{}/dlc.manifest", game.data_dir()),
        manifest.as_bytes(),
    );
    game.dlc = paths.clone();
    let mut app = app_with(&dir, vec![game]);
    app.dispatch(Action::InstallDlc);
    let mut h = harness(app);
    h.run();
    assert!(h.query_by_label("DLC — Kart window").is_some());
    h.get_by_label("First.nsp — 0100152000023001 v1").click();
    h.run();
    h.get_by_label("Third.xci — 0100152000023003 v1")
        .click_modifiers(egui::Modifiers::COMMAND);
    h.run();
    h.get_by_label("Remove").click();
    h.run_steps(3);
    settle(&mut h.state_mut().app);
    assert_eq!(h.state().app.lib.games[0].dlc, vec![paths[1].clone()]);
}

#[test]
fn updates_window_chooses_and_builds() {
    let (_t, dir) = tmp();
    let mut game = built_game(&dir);
    let (older, newer) = (
        format!("{dir}/Update [v65536].nsp"),
        format!("{dir}/Update [v262144].nsp"),
    );
    write(&older, b"PFS0");
    write(&newer, b"PFS0");
    let u = |p: &str, v: i64, d: &str| GameUpdate {
        path: p.into(),
        title_id: "01001f5010dfa000".into(),
        version: v,
        display: d.into(),
    };
    game.updates = vec![u(&older, 65536, "1.0.1"), u(&newer, 262144, "1.1.1")];
    game.built_update = newer.clone();
    let mut app = app_with(&dir, vec![game]);
    app.dispatch(Action::InstallUpdates);
    let mut h = harness(app);
    h.run();
    assert!(
        h.query_by_label("Update [v262144].nsp — 1.1.1 (v262144) — applied")
            .is_some()
    );
    // the build is what is chosen: Build now does nothing
    h.get_by_label("Build now").click();
    h.run();
    assert_eq!(h.state().app.surfaces.len(), 1);
    h.get_by_role_and_label(Role::ComboBox, "Version to build")
        .click();
    h.run();
    h.get_by_label("1.0.1 (v65536) — Update [v65536].nsp")
        .click();
    h.run();
    assert_eq!(h.state().app.lib.games[0].update_choice, older);
    h.get_by_label("Build now").click();
    h.run_steps(3);
    let left = h.state().app.surfaces.len();
    assert_eq!(
        left,
        0,
        "Build now closes the window and builds (pending: {:?})",
        h.state()
            .app
            .update_pending_text(&h.state().app.lib.games[0])
    );
    settle(&mut h.state_mut().app);
}

#[test]
fn title_bar_glyph_follows_the_accent() {
    assert_eq!(theme::palette("dark", "lime").accent, theme::hex("#b5f36b"));
    assert_eq!(
        theme::palette("light", "rose").accent,
        theme::hex("#ab2456")
    );
    assert_eq!(
        theme::palette("light", "nonsense").accent,
        theme::hex("#38711b")
    );
}

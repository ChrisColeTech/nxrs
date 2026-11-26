//! One scaffold for every dialog (owner, 2026-11-26; style guide §5): the shared header's X on child
//! windows and popovers, the shared footer (a message's ghost Close, a list window's leading verbs,
//! Close and its one filled action), a list's empty state as a status line, no Done on a Live popover.

mod common;

use common::*;
use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, HarnessBuilder};
use nxrs_launcher::flows::{Action, App};
use nxrs_launcher::services::jobs::noop;
use nxrs_launcher::ui::shell::Shell;

fn harness(app: App) -> Harness<'static, Shell> {
    HarnessBuilder::default()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, s: &mut Shell| s.frame(ui), Shell::new(app, noop()))
}

fn opened(dir: &str, action: Action) -> Harness<'static, Shell> {
    let mut app = app_with(dir, vec![built_game(dir)]);
    app.interactive = true;
    app.dispatch(action);
    let mut h = harness(app);
    h.run_steps(4);
    h
}

#[test]
fn a_message_closes_with_the_shared_footer_close() {
    let (_t, dir) = tmp();
    let mut h = opened(&dir, Action::About);
    assert!(h.query_by_label("Close nxrs").is_some(), "the header's X");
    // the main window's caption has a Close too; the message's is the last one drawn
    let footer = h.query_all_by_role_and_label(Role::Button, "Close").last();
    footer.expect("the footer's Close").click();
    h.run_steps(4);
    assert!(h.state().app.surfaces.is_empty());
}

#[test]
fn a_list_window_has_the_shared_footer_and_its_empty_state() {
    let (_t, dir) = tmp();
    let h = opened(&dir, Action::InstallDlc);
    for label in ["Close DLC — Kart", "Remove", "Add DLC…"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    // the caption's Close and the footer's
    assert_eq!(
        h.query_all_by_role_and_label(Role::Button, "Close").count(),
        2
    );
    assert!(
        h.query_by_label("No DLC is installed for this game.")
            .is_some()
    );
    let h = opened(&dir, Action::InstallUpdates);
    for label in ["Remove", "Build now", "Add update…"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(
        h.query_by_label("No update is installed for this game.")
            .is_some()
    );
}

#[test]
fn the_filter_popover_has_the_shared_header_and_no_done() {
    let (_t, dir) = tmp();
    let mut app = app_with(&dir, vec![built_game(&dir)]);
    app.filters.open = true;
    let mut h = harness(app);
    h.run_steps(4);
    assert!(h.query_by_label("Clear filters").is_some());
    assert!(h.query_by_label("Done").is_none());
    h.get_by_label("Close Search & Filter").click();
    h.run_steps(4);
    assert!(!h.state().app.filters.open);
}

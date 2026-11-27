//! One scaffold for every dialog (owner, 2026-11-26; style guide §5): the header's X is the only
//! dismissal (no Close, Cancel, No or Yes in any footer); a message has one filled OK, a question
//! one verb, a rail window its commit, a list window its verbs and its filled Add; a message is
//! titled by its task and headed by itself; a list's empty state as a status line; no Done on a
//! Live popover.

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

/// The only buttons named like a dismissal: the main window caption's Close. No footer has one.
fn no_footer_dismissal(h: &Harness<'static, Shell>) {
    let count = |name| h.query_all_by_role_and_label(Role::Button, name).count();
    assert_eq!(count("Close"), 1, "only the main window's caption");
    for name in ["Cancel", "No", "Yes", "Done"] {
        assert_eq!(count(name), 0, "{name}");
    }
}

#[test]
fn a_message_has_the_header_and_one_filled_ok() {
    let (_t, dir) = tmp();
    let mut h = opened(&dir, Action::About);
    // the window names its task, never the app alone; the heading is the message's own
    assert!(
        h.query_by_label("Close About nxrs").is_some(),
        "titled About nxrs"
    );
    assert!(h.query_by_label("Close nxrs").is_none());
    no_footer_dismissal(&h);
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    assert!(h.state().app.surfaces.is_empty());
    // the X closes it too
    let mut h = opened(&dir, Action::About);
    h.get_by_label("Close About nxrs").click();
    h.run_steps(4);
    assert!(h.state().app.surfaces.is_empty());
}

#[test]
fn an_error_is_titled_by_its_task_and_headed_by_itself() {
    let (_t, dir) = tmp();
    let notes = format!("{dir}/notes.txt");
    write(&notes, b"not a game");
    let mut app = app_with(&dir, vec![built_game(&dir)]);
    app.interactive = true;
    app.import_source(&notes, true, "");
    let mut h = harness(app);
    h.run_steps(4);
    assert!(
        h.query_by_label("Close Add game").is_some(),
        "titled Add game"
    );
    assert!(h.query_by_label("Cannot add game").is_some(), "the heading");
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    assert!(h.state().app.surfaces.is_empty());
}

#[test]
fn a_question_has_one_verb_and_its_x_answers_no() {
    let (_t, dir) = tmp();
    let mut app = app_with(&dir, vec![built_game(&dir)]);
    app.ask_close();
    let mut h = harness(app);
    h.run_steps(4);
    assert!(
        h.query_by_label("Cancel the build and close the launcher?")
            .is_some()
    );
    assert!(h.query_by_label("Stop build and close").is_some());
    no_footer_dismissal(&h);
    h.get_by_label("Close Build in progress").click();
    h.run_steps(4);
    assert!(h.state().app.surfaces.is_empty() && !h.state().app.closing);
}

#[test]
fn rail_windows_end_with_their_commit_only() {
    let (_t, dir) = tmp();
    let h = opened(&dir, Action::Settings(0));
    assert!(h.query_by_label("Apply").is_some());
    no_footer_dismissal(&h);
    let h = opened(&dir, Action::Properties(0));
    assert!(h.query_by_label("Apply").is_some());
    no_footer_dismissal(&h);
}

#[test]
fn a_list_window_has_the_shared_footer_and_its_empty_state() {
    let (_t, dir) = tmp();
    let h = opened(&dir, Action::InstallDlc);
    for label in ["Close DLC — Kart", "Remove", "Add DLC…"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    no_footer_dismissal(&h);
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

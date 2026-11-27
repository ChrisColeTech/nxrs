//! One Play and one Build (owner, 2026-11-27): every menu has exactly one of each and no web entries;
//! Play runs the one version that can play and asks Native or Web only when both can; Build asks.
//! The question is the shared question box: its heading names the game, Native its filled verb,
//! Web the ghost verb at the leading side, the X answering nothing; Web builds through the real shell.

mod common;

use common::*;
use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use nxrs_launcher::constants::words_target as t;
use nxrs_launcher::flows::surface::Surface;
use nxrs_launcher::flows::{Action, App, Outcome};
use nxrs_launcher::model::game::Game;
use nxrs_launcher::model::web_build::web_dir;
use nxrs_launcher::services::browser::Browser;
use nxrs_launcher::services::jobs::noop;
use nxrs_launcher::ui::chrome::menu_model::{Item, context, menu};
use nxrs_launcher::ui::shell::Shell;

fn harness(app: App) -> Harness<'static, Shell> {
    HarnessBuilder::default()
        .with_size([1280.0, 800.0])
        .build_ui_state(|ui, s: &mut Shell| s.frame(ui), Shell::new(app, noop()))
}

fn labels(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Command { label, .. } => Some(label.clone()),
            Item::Separator => None,
        })
        .collect()
}

/// Play's or Build's question is the one open surface: its filled verb's answer.
fn asked(app: &App) -> Option<Action> {
    match &app.surfaces[..] {
        [Surface::Question { yes, .. }] => Some(yes.clone()),
        _ => None,
    }
}

/// A finished web build of `g` (as nxrs-build writes it) and a browser to play it in.
fn with_web_build(app: &mut App, g: &Game) {
    let root = web_dir(g);
    write(
        &format!("{root}/build.json"),
        br#"{"format":1,"kind":"nxrs-web","title":"Kart","entry":"index.html","data":"Kart_data"}"#,
    );
    write(&format!("{root}/game.wasm"), b"\0asm");
    std::fs::create_dir_all(format!("{root}/Kart_data")).unwrap();
    app.tools.browser = Some(Browser {
        name: "Test".into(),
        program: FAKE.into(),
    });
    app.refresh_facts();
}

fn disabled(h: &Harness<'static, Shell>, label: &str) -> bool {
    h.get_by_role_and_label(Role::Button, label)
        .accesskit_node()
        .is_disabled()
}

#[test]
fn one_play_and_one_build_in_every_menu() {
    let (_t, dir) = tmp();
    let app = app_with(&dir, vec![built_game(&dir)]);
    for items in [menu(&app, "Game"), context(&app)] {
        let l = labels(&items);
        let count = |s: &str| l.iter().filter(|x| x.as_str() == s).count();
        assert_eq!((count("Play"), count("Build")), (1, 1), "{l:?}");
        assert!(
            !l.iter().any(|x| x.contains("browser") || x.contains("web")),
            "{l:?}"
        );
    }
}

#[test]
fn play_asks_only_when_both_versions_can_play() {
    let (_t, dir) = tmp();
    let g = built_game(&dir);
    let mut app = app_with(&dir, vec![g.clone()]);
    // native only: Play (menu, card, Enter) runs it, no question
    for action in [
        Action::Play,
        Action::CardPrimary("g".into()),
        Action::Activate,
    ] {
        app.dispatch(action.clone());
        assert_ne!(asked(&app), Some(Action::PlayNative), "{action:?}");
        app.surfaces.clear();
    }
    // both: Play asks, once
    with_web_build(&mut app, &g);
    app.dispatch(Action::Play);
    app.dispatch(Action::Play);
    assert_eq!(asked(&app), Some(Action::PlayNative));
    app.surfaces.clear();
    // web only: Play serves it and opens the browser, no question
    std::fs::remove_file(g.executable()).unwrap();
    app.refresh_facts();
    assert_eq!(
        app.why_not("g", false, false),
        Some(t::NO_NATIVE_BUILD.into())
    );
    app.dispatch(Action::Play);
    assert!(app.surfaces.is_empty());
    assert!(app.job_label.contains("/kart/"), "{}", app.job_label);
    // nothing can play: Play does nothing
    app.demo = true;
    app.dispatch(Action::Play);
    assert!(app.surfaces.is_empty());
}

#[test]
fn build_asks_and_web_builds() {
    let (_t, dir) = tmp();
    let mut app = app_with(&dir, vec![built_game(&dir)]);
    app.dispatch(Action::Build);
    app.dispatch(Action::Build);
    assert_eq!(asked(&app), Some(Action::BuildNative), "asked once");
    let mut h = harness(app);
    h.run_steps(4);
    // the window names the task; the heading is the question, naming the game; no Cancel or No
    assert!(h.query_by_label("Close Build").is_some());
    assert!(h.query_by_label_contains("Build Kart natively").is_some());
    assert!(h.query_by_label("Cancel").is_none() && h.query_by_label("No").is_none());
    // built and up to date: Native is disabled (its reason its tooltip), Web can build
    assert!(disabled(&h, t::NATIVE) && !disabled(&h, t::WEB));
    h.get_by_role_and_label(Role::Button, t::WEB).click();
    h.run_steps(4);
    let app = &mut h.state_mut().app;
    assert!(app.surfaces.is_empty());
    settle(app);
    assert_eq!(app.outcomes, vec![Outcome::Smoke(true)]);
    assert!(app.facts_of("g").web_build, "Web ran the web build");
}

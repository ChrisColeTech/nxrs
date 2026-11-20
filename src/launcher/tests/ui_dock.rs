//! The docked build output as the family's split: its sash by its accessible name, dragged 1:1 within
//! the pane table, reset by a double click, the size kept across hiding it and given up to a small
//! window only in paint.

mod common;

use common::*;
use egui::accesskit::Role;
use egui::{Event, PointerButton, Pos2, pos2, vec2};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, HarnessBuilder};
use nxrs_launcher::constants::panes as p;
use nxrs_launcher::flows::App;
use nxrs_launcher::services::jobs::noop;
use nxrs_launcher::ui::shell::Shell;

fn harness(app: App, size: [f32; 2]) -> Harness<'static, Shell> {
    HarnessBuilder::default()
        .with_size(size)
        .build_ui_state(|ui, s: &mut Shell| s.frame(ui), Shell::new(app, noop()))
}

fn press(h: &mut Harness<'_, Shell>, at: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    });
    h.run_steps(2);
}

fn moved(h: &mut Harness<'_, Shell>, at: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(at));
    h.run_steps(2);
}

fn sash_at(h: &Harness<'_, Shell>) -> Pos2 {
    h.get_by_role_and_label(Role::Splitter, p::SASH_CONSOLE)
        .rect()
        .center()
}

#[test]
fn the_build_output_resizes_like_the_family_split() {
    let (_t, dir) = tmp();
    let mut app = App::new(&format!("{dir}/library.json"), true, noop());
    app.open();
    app.lib.console.visible = true;
    let mut h = harness(app, [1280.0, 800.0]);
    h.run_steps(4);
    let start = sash_at(&h);
    assert_eq!(h.state().app.lib.console.size, p::CONSOLE_BOTTOM.preferred);
    // up 100 points: 100 points taller, 1:1
    moved(&mut h, start);
    press(&mut h, start, true);
    moved(&mut h, start - vec2(0.0, 100.0));
    press(&mut h, start - vec2(0.0, 100.0), false);
    assert_eq!(
        h.state().app.lib.console.size,
        p::CONSOLE_BOTTOM.preferred + 100.0
    );
    // the seam followed the pointer
    assert!((sash_at(&h).y - (start.y - 100.0)).abs() < 1.0);
    // dragged far past the table: the greatest size, no further
    let at = sash_at(&h);
    press(&mut h, at, true);
    moved(&mut h, pos2(at.x, 0.0));
    press(&mut h, pos2(at.x, 0.0), false);
    assert_eq!(h.state().app.lib.console.size, p::CONSOLE_BOTTOM.max);
    // a double click gives the first size back
    let at = sash_at(&h);
    moved(&mut h, at);
    // (one frame: the harness moves its clock half a second a step, past the double-click delay)
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
    }
    h.run_steps(2);
    assert_eq!(h.state().app.lib.console.size, p::CONSOLE_BOTTOM.preferred);
    // hidden and shown again: the saved size, not the Qt reset
    h.state_mut().app.lib.console.size = 300.0;
    h.state_mut().app.lib.console.visible = false;
    h.run_steps(20);
    assert!(
        h.query_by_role_and_label(Role::Splitter, p::SASH_CONSOLE)
            .is_none()
    );
    h.state_mut().app.lib.console.visible = true;
    h.run_steps(20);
    assert_eq!(h.state().app.lib.console.size, 300.0);
}

#[test]
fn a_small_window_refits_the_paint_not_the_saved_size() {
    let (_t, dir) = tmp();
    let mut app = App::new(&format!("{dir}/library.json"), true, noop());
    app.open();
    app.lib.console.visible = true;
    app.lib.console.size = p::CONSOLE_BOTTOM.max;
    let mut h = harness(app, [1000.0, 500.0]);
    h.run_steps(4);
    let sash = sash_at(&h);
    // the library above the seam keeps its floor
    let commands_bottom = h.get_by_label("Search & Filter").rect().bottom();
    assert!(
        sash.y - commands_bottom >= p::LIBRARY_MIN_HEIGHT - 20.0,
        "{sash:?}"
    );
    assert_eq!(h.state().app.lib.console.size, p::CONSOLE_BOTTOM.max);
}

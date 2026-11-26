//! Message and question boxes (`QMessageBox`'s answers) in the family's scaffold: the shared header
//! and close, the text, and the shared footer (`authored::footer`: a hairline, the ghost button, the
//! one filled primary last): Close, or No / Yes (the style guide's footer language; Enter answers the
//! default). A question with a tick box: a Group heading (its title, the question in its info mark)
//! and the tick box under it with its own info mark.

use super::Reply;
use crate::constants::{child_window as c, popups as p, words_settings as w};
use crate::flows::Action;
use crate::flows::surface::Check;
use crate::ui::popups::{authored, child_window::ChildSpec};
use crate::ui::theme;
use crate::ui::widgets::page;
use crate::ui::widgets::rail::Icon;
use egui::Ui;

/// The Group heading, its Row and the footer.
const CHECKED_BODY: f32 = 130.0;

/// Sized to its text: about 62 characters a line at its width; a question with a tick box to its
/// Group and Row.
pub fn spec<'a>(key: &'a str, title: &'a str, text: &str, checked: bool) -> ChildSpec<'a> {
    let lines: f32 = text
        .lines()
        .map(|l| (l.chars().count() as f32 / 62.0).ceil().max(1.0))
        .sum();
    let body = if checked {
        CHECKED_BODY
    } else {
        (lines * 19.0 + 70.0).clamp(110.0, 460.0)
    };
    ChildSpec::work(key, title, c::MESSAGE_WIDTH, body)
        .least(c::MESSAGE_LEAST, body.min(160.0))
        .popup()
}

/// The text, scrolling when long, at the window's inset.
fn text_area(ui: &mut Ui, text: &str) {
    egui::ScrollArea::vertical()
        .max_height(authored::body_height(ui))
        .show(ui, |ui| {
            ui.add(egui::Label::new(text).wrap().selectable(true));
        });
}

/// The shared footer at the bottom of the window: (the ghost button, the primary) pressed; Enter
/// presses the primary unless `enter_is_ghost`.
fn footer(ui: &mut Ui, primary: &str, ghost: &str, enter_is_ghost: bool) -> (bool, bool) {
    ui.add_space((ui.available_height() - p::FOOTER).max(0.0));
    let pal = theme::of(ui.ctx());
    let (ghosted, pressed) = authored::footer(ui, &pal, primary, true, ghost);
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    (
        ghosted || (enter && enter_is_ghost),
        pressed || (enter && !enter_is_ghost),
    )
}

pub fn message(ui: &mut Ui, text: &str) -> Reply {
    text_area(ui, text);
    let (_, close) = footer(ui, w::CLOSE, "", false);
    Reply {
        close,
        ..Reply::default()
    }
}

pub fn question(
    ui: &mut Ui,
    text: &str,
    (yes, no): (&Action, Option<&Action>),
    default_no: bool,
    check: Option<&mut Check>,
) -> Reply {
    let (said_no, said_yes) = match check {
        Some(c) => {
            page::group(ui, Icon::General, &c.heading, text);
            // one control, no form: the tick box at the inset, its explanation in the info mark after it
            ui.horizontal(|ui| {
                ui.checkbox(&mut c.on, c.label.as_str());
                page::info(ui, &c.tip);
            });
            let (said_no, said_yes) = footer(ui, w::YES, w::NO, default_no);
            let chosen = if c.on { c.yes_on.clone() } else { yes.clone() };
            (said_no, said_yes.then_some(chosen))
        }
        None => {
            text_area(ui, text);
            let (said_no, said_yes) = footer(ui, w::YES, w::NO, default_no);
            (said_no, said_yes.then(|| yes.clone()))
        }
    };
    let mut reply = Reply::default();
    if let Some(action) = said_yes {
        reply.actions.push(action);
        reply.close = true;
    } else if said_no {
        reply.actions.extend(no.cloned());
        reply.close = true;
    }
    reply
}

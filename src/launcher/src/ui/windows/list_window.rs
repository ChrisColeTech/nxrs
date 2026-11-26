//! What the DLC and Updates windows share: the list, the hint, the outcome, the busy bar and the
//! buttons row.

use crate::constants::popups::FOOTER;
use crate::flows::surface::ListWindow;
use crate::ui::popups::authored;
use crate::ui::theme::Palette;
use crate::ui::widgets::{button, form, pick_list};
use egui::{Align, Layout, Ui, vec2};

pub struct Buttons<'a> {
    pub add: &'a str,
    pub add_tip: &'a str,
    pub remove_tip: &'a str,
    /// Build now (Updates only): (label, tip, enabled)
    pub build: Option<(&'a str, &'a str, bool)>,
    /// the list's state under it, before the outcome (locked, an update pending)
    pub lines: &'a [&'a str],
}

#[derive(Default)]
pub struct Pressed {
    pub add: bool,
    pub remove: bool,
    pub build: bool,
    pub close: bool,
    pub menu: Option<(usize, usize)>,
}

pub fn show(
    ui: &mut Ui,
    p: &Palette,
    w: &mut ListWindow,
    list: (&str, &[(String, String)]),
    (busy, editable): (bool, bool),
    b: &Buttons,
    menu: &dyn Fn(usize, usize) -> Vec<(String, bool)>,
) -> Pressed {
    let mut pressed = Pressed::default();
    // laid out from the bottom: the buttons, the outcome and the notes always show; the list takes the rest
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        // the shared footer: the list's own verbs lead, Close, the one filled action last
        let (mut remove, mut build) = (false, false);
        let can_remove = editable && !w.picks.rows.is_empty();
        let lead = |ui: &mut Ui| {
            let r = button::ghost(ui, crate::constants::words_dlc::REMOVE, can_remove);
            remove = button::tip(r, b.remove_tip).clicked();
            if let Some((label, tip, enabled)) = b.build {
                build = button::tip(button::ghost(ui, label, editable && enabled), tip).clicked();
            }
        };
        let size = vec2(ui.available_width(), FOOTER);
        let close = crate::constants::words_settings::CLOSE;
        let (closed, added) = ui
            .allocate_ui_with_layout(size, Layout::top_down(Align::Min), |ui| {
                authored::footer_bar(ui, p, lead, close, Some((b.add, editable, b.add_tip)))
            })
            .inner;
        (pressed.remove, pressed.build) = (remove, build);
        (pressed.close, pressed.add) = (closed, added);
        ui.add_space(4.0);
        if busy {
            form::busy(ui, p);
        }
        if let Some((text, error)) = &w.status {
            form::status(ui, p, text, *error);
        }
        for line in b.lines.iter().rev().filter(|l| !l.is_empty()) {
            form::status(ui, p, line, false);
        }
        ui.add_space(4.0);
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            let height = ui.available_height();
            let out = pick_list::show(ui, p, list.0, list.1, &mut w.picks, height - 6.0, menu);
            pressed.menu = out.menu;
            pressed.remove |= out.delete && editable;
        });
    });
    pressed
}

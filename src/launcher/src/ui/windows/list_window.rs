//! What the DLC and Updates windows share: the list, the hint, the outcome, the busy bar and the
//! buttons row.

use crate::flows::surface::ListWindow;
use crate::ui::theme::Palette;
use crate::ui::widgets::{button, form, pick_list};
use egui::{Align, Layout, Ui};

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
        // every window's one filled action is last, at the right; the list's own verbs lead
        ui.horizontal(|ui| {
            let remove = button::ghost(
                ui,
                crate::constants::words_dlc::REMOVE,
                editable && !w.picks.rows.is_empty(),
            );
            pressed.remove |= button::tip(remove, b.remove_tip).clicked();
            if let Some((label, tip, enabled)) = b.build {
                pressed.build =
                    button::tip(button::ghost(ui, label, editable && enabled), tip).clicked();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                pressed.add =
                    button::tip(button::primary(ui, p, b.add, editable), b.add_tip).clicked();
                pressed.close =
                    button::ghost(ui, crate::constants::words_settings::CLOSE, true).clicked();
            });
        });
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

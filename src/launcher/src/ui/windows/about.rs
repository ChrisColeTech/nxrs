//! Help > About nxrs (owner, 2026-11-27: "the app icon and name"): the message's body header with the
//! app's own icon (the title bar's) in place of the line-glyph tile, the name and the build's version
//! beside it, no text; the footer's ghost Website and GitHub at the leading side and the filled OK
//! last. OK, Enter, the X and Escape close it.

use super::{Reply, message};
use crate::constants::{popups as p, theme as tokens, words_contexts as c, words_library as w};
use crate::flows::Action;
use crate::ui::popups::{authored, child_window::ChildSpec};
use crate::ui::widgets::{button, page};
use crate::ui::{icons, theme};
use egui::{Sense, Ui, vec2};

/// Sized as a message with a heading and no text.
pub fn spec<'a>(key: &'a str, title: &'a str) -> ChildSpec<'a> {
    message::spec((key, title), (c::ABOUT_HEADING, ""), false)
}

pub fn show(ui: &mut Ui) -> Reply {
    let pal = theme::of(ui.ctx());
    // the body header's spacing and sizes (`page::heading`), the app icon in the tile's place
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let (tile, _) = ui.allocate_exact_size(vec2(page::TILE, page::TILE), Sense::hover());
        icons::accent_glyph(ui.painter(), tile, pal.accent, pal.surface);
        ui.add_space(4.0);
        let name = egui::RichText::new(c::ABOUT_HEADING)
            .font(theme::semibold(16.0))
            .color(pal.text);
        ui.label(name);
        let version = egui::RichText::new(w::ABOUT_VERSION)
            .font(theme::sans(tokens::FONT_SMALL))
            .color(pal.muted);
        ui.label(version);
    });
    ui.add_space((ui.available_height() - p::FOOTER).max(0.0));
    let mut actions = Vec::new();
    let lead = |ui: &mut Ui| {
        for (label, address) in w::ABOUT_LINKS {
            if button::ghost(ui, label, true).clicked() {
                actions.push(Action::OpenUrl(address.into()));
            }
        }
    };
    let ok = authored::footer_bar(
        ui,
        &pal,
        lead,
        (crate::constants::words_settings::OK, true, ""),
    );
    Reply {
        close: ok || ui.input(|i| i.key_pressed(egui::Key::Enter)),
        actions,
    }
}

//! Settings' pages (`showSettings`).

use crate::constants::{engine as e, theme as tokens, words_settings as w};
use crate::flows::App;
use crate::flows::settings::SettingsDraft;
use crate::model::path_text::display;
use crate::ui::theme;
use crate::ui::widgets::field::{self, Browse};
use crate::ui::widgets::{form, select};
use egui::{CornerRadius, Sense, Ui, vec2};

pub fn engine(ui: &mut Ui, app: &mut App, s: &mut SettingsDraft) {
    let p = theme::of(ui.ctx());
    let exe: (&str, &[&str]) = (
        e::NXRS_BUILD_NAME,
        if cfg!(windows) { &["exe"] } else { &["*"] },
    );
    form::row(ui, w::ENGINE_ROW, |ui| {
        field::path(
            ui,
            &mut s.nxrs_build,
            w::ENGINE_ROW,
            w::NXRS_PICK,
            Browse::File(Some(exe)),
            w::NXRS_PLACEHOLDER,
        );
    });
    form::selectable(ui, &p, &app.health_text());
    ui.add_space(10.0);
    form::row(ui, w::CHECKS_ROW, |ui| {
        ui.vertical(|ui| {
            form::selectable(ui, &p, &app.doctor_text);
            let can = !app.tools.nxrs_build.is_empty() && !app.doctor_running;
            if ui
                .add_enabled(can, egui::Button::new(w::CHECK_AGAIN))
                .clicked()
            {
                app.run_doctor();
            }
        });
    });
}

pub fn builds(ui: &mut Ui, s: &mut SettingsDraft) {
    form::row(ui, "Build location", |ui| {
        field::path(
            ui,
            &mut s.output_root,
            "Build location",
            "Choose default output folder",
            Browse::Folder,
            w::OUTPUT_PLACEHOLDER,
        );
    });
    form::row(ui, "Compiler workers", |ui| {
        ui.add(egui::DragValue::new(&mut s.jobs).range(1..=64).speed(0.1));
    });
}

pub fn controller(ui: &mut Ui, s: &mut SettingsDraft) {
    let p = theme::of(ui.ctx());
    let layouts = select::words(&[
        (w::LAYOUT_SWITCH, "switch".to_string()),
        (w::LAYOUT_XBOX, "xbox".to_string()),
    ]);
    form::row(ui, "Button layout", |ui| {
        select::select(ui, "Button layout", &mut s.button_layout, &layouts);
    });
    form::note(ui, &p, w::CONTROLLER_NOTE);
}

pub fn appearance(ui: &mut Ui, app: &mut App, s: &mut SettingsDraft) {
    let p = theme::of(ui.ctx());
    let mut changed = false;
    let modes = select::words(&[("Dark", "dark".to_string()), ("Light", "light".to_string())]);
    form::row(ui, "Color mode", |ui| {
        changed |= select::select(ui, "Color mode", &mut s.color_mode, &modes)
    });
    form::row(ui, "Accent color", |ui| {
        let before = s.accent.clone();
        let shown = tokens::ACCENTS
            .iter()
            .find(|a| a.0 == s.accent)
            .map_or("Lime", |a| a.1);
        let width = ui.available_width();
        egui::ComboBox::from_id_salt("Accent color")
            .selected_text(shown)
            .width(width)
            .show_ui(ui, |ui| {
                for (id, label, ..) in tokens::ACCENTS {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        ui.painter().rect_filled(
                            r,
                            CornerRadius::ZERO,
                            theme::palette(&s.color_mode, id).accent,
                        );
                        ui.selectable_value(&mut s.accent, id.to_string(), label);
                    });
                }
            });
        changed |= before != s.accent;
    });
    ui.checkbox(&mut s.show_background, w::BACKDROP);
    ui.add_space(8.0);
    form::selectable(
        ui,
        &p,
        &format!("{}{}", w::LIBRARY_FILE, display(&app.lib.path)),
    );
    if changed {
        // a live preview; Cancel restores the saved one
        app.appearance = (s.color_mode.clone(), s.accent.clone());
    }
}

pub fn play(ui: &mut Ui, s: &mut SettingsDraft) {
    let p = theme::of(ui.ctx());
    let modes = select::words(&[(w::SCREEN_TV, true), (w::SCREEN_HANDHELD, false)]);
    form::row(ui, "Screen mode", |ui| {
        select::select(ui, "Screen mode", &mut s.docked, &modes);
    });
    ui.checkbox(&mut s.pause, w::PAUSE);
    ui.checkbox(&mut s.mute, w::MUTE);
    ui.checkbox(&mut s.ask_user, w::ASK_USER);
    ui.add_space(8.0);
    form::note(ui, &p, w::PLAY_NOTE);
}

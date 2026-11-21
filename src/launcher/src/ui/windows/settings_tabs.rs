//! Settings' sections (`showSettings`) in the family's page grammar: each a Group or two of labelled
//! rows, the explanations in the info marks' tooltips.

use crate::constants::{engine as e, theme as tokens, words_settings as w};
use crate::flows::App;
use crate::flows::settings::SettingsDraft;
use crate::model::path_text::display;
use crate::ui::theme;
use crate::ui::widgets::field::{self, Browse};
use crate::ui::widgets::page::{group, readout, row, rule};
use crate::ui::widgets::rail::Icon;
use crate::ui::widgets::select;
use egui::{CornerRadius, Sense, Ui, vec2};

const ENGINE_TIP: &str = "The nxrs-build this launcher runs. Left as found, it is the engine beside the launcher (a release's nxrs-engine folder); choose another for a development build.";
const CHECKS_TIP: &str = "What nxrs-build doctor checks on this machine: the CPU, the compiler, the engine libraries, Vulkan, free disk and the keys.";
const WORKERS_TIP: &str = "How many compiler processes a build runs at once.";
const BACKGROUND_TIP: &str = "The selected game's artwork, dimmed behind the gallery.";
const PROFILES_TIP: &str = "When several profiles exist, the game asks who is playing.";

pub fn engine(ui: &mut Ui, app: &mut App, s: &mut SettingsDraft) {
    let exe: (&str, &[&str]) = (
        e::NXRS_BUILD_NAME,
        if cfg!(windows) { &["exe"] } else { &["*"] },
    );
    group(ui, Icon::Engine, w::ENGINE_ROW, ENGINE_TIP);
    row(ui, "nxrs-build", ENGINE_TIP, |ui| {
        let browse = Browse::File(Some(exe));
        field::path(
            ui,
            &mut s.nxrs_build,
            w::ENGINE_ROW,
            w::NXRS_PICK,
            browse,
            w::NXRS_PLACEHOLDER,
        );
    });
    row(ui, "Status", "", |ui| readout(ui, &app.health_text()));
    rule(ui);
    group(ui, Icon::Engine, w::CHECKS_ROW, CHECKS_TIP);
    readout(ui, &app.doctor_text);
    ui.add_space(8.0);
    let can = !app.tools.nxrs_build.is_empty() && !app.doctor_running;
    if ui
        .add_enabled(can, egui::Button::new(w::CHECK_AGAIN))
        .clicked()
    {
        app.run_doctor();
    }
}

pub fn builds(ui: &mut Ui, s: &mut SettingsDraft) {
    group(
        ui,
        Icon::Build,
        "Build defaults",
        "Where new builds go and how hard a build works.",
    );
    row(ui, "Build location", w::OUTPUT_PLACEHOLDER, |ui| {
        let browse = Browse::Folder;
        let pick = "Choose default output folder";
        field::path(
            ui,
            &mut s.output_root,
            "Build location",
            pick,
            browse,
            w::OUTPUT_PLACEHOLDER,
        );
    });
    row(ui, "Compiler workers", WORKERS_TIP, |ui| {
        ui.add(egui::DragValue::new(&mut s.jobs).range(1..=64).speed(0.1));
    });
}

pub fn controller(ui: &mut Ui, s: &mut SettingsDraft) {
    let layouts = select::words(&[
        (w::LAYOUT_SWITCH, "switch".to_string()),
        (w::LAYOUT_XBOX, "xbox".to_string()),
    ]);
    group(ui, Icon::Controller, "Controller", w::CONTROLLER_NOTE);
    row(ui, "Button layout", w::CONTROLLER_NOTE, |ui| {
        select::select(ui, "Button layout", &mut s.button_layout, &layouts);
    });
}

pub fn appearance(ui: &mut Ui, app: &mut App, s: &mut SettingsDraft) {
    let mut changed = false;
    let modes = select::words(&[("Dark", "dark".to_string()), ("Light", "light".to_string())]);
    group(
        ui,
        Icon::Appearance,
        "Appearance",
        "The launcher's colors; they preview live.",
    );
    row(ui, "Color mode", "", |ui| {
        changed |= select::select(ui, "Color mode", &mut s.color_mode, &modes)
    });
    row(ui, "Accent color", "", |ui| changed |= accent(ui, s));
    row(ui, "Background", BACKGROUND_TIP, |ui| {
        ui.checkbox(&mut s.show_background, w::BACKDROP);
    });
    rule(ui);
    group(ui, Icon::Appearance, "Library", "");
    row(ui, "Library file", "", |ui| {
        readout(ui, &display(&app.lib.path))
    });
    if changed {
        // a live preview; Cancel restores the saved one
        app.appearance = (s.color_mode.clone(), s.accent.clone());
    }
}

/// The accent picker: each choice with its swatch. True when it changed.
fn accent(ui: &mut Ui, s: &mut SettingsDraft) -> bool {
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
                    let fill = theme::palette(&s.color_mode, id).accent;
                    ui.painter().rect_filled(r, CornerRadius::ZERO, fill);
                    ui.selectable_value(&mut s.accent, id.to_string(), label);
                });
            }
        });
    before != s.accent
}

pub fn play(ui: &mut Ui, s: &mut SettingsDraft) {
    let modes = select::words(&[(w::SCREEN_TV, true), (w::SCREEN_HANDHELD, false)]);
    group(ui, Icon::Play, "Screen", w::PLAY_NOTE);
    row(ui, "Screen mode", w::SCREEN_TIP, |ui| {
        select::select(ui, "Screen mode", &mut s.docked, &modes);
    });
    rule(ui);
    group(ui, Icon::Play, "While playing", w::PLAY_NOTE);
    row(ui, "In the background", w::PLAY_NOTE, |ui| {
        ui.vertical(|ui| {
            ui.checkbox(&mut s.pause, "Pause the game");
            ui.checkbox(&mut s.mute, "Mute the game");
        });
    });
    row(ui, "Profiles", PROFILES_TIP, |ui| {
        ui.checkbox(&mut s.ask_user, "Ask who is playing");
    });
}

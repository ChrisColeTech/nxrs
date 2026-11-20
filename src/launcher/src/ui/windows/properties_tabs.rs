//! Properties' General, Build, Runtime and Artwork pages (`editGame`).

use crate::constants::{words_library as l, words_settings as w};
use crate::flows::App;
use crate::flows::properties::PropertiesDraft;
use crate::ui::theme;
use crate::ui::widgets::field::{self, Browse};
use crate::ui::widgets::{form, select};
use egui::Ui;

pub fn general(ui: &mut Ui, app: &App, d: &mut PropertiesDraft) {
    let p = theme::of(ui.ctx());
    form::row(ui, "Display name", |ui| {
        field::text(ui, &mut d.title, "", "Display name");
    });
    ui.add_enabled_ui(!d.locked, |ui| {
        form::row(ui, "Source", |ui| {
            field::path(
                ui,
                &mut d.source,
                "Source",
                "Choose source",
                Browse::File(Some(l::GAME_FILTER)),
                "",
            );
        });
        form::row(ui, "", |ui| {
            if ui.button(w::CHOOSE_FOLDER).clicked()
                && let Some(folder) =
                    crate::flows::pick("Choose extracted game folder", &d.source, Browse::Folder)
            {
                d.source = app.native(&folder);
            }
        });
    });
    form::value(ui, "Format", d.game.format());
    let version = app.version_of(&d.game);
    if !version.is_empty() {
        form::value(ui, "Version", &version);
    }
    form::value(ui, "Status", &d.status);
    form::note(ui, &p, w::PATHS_NOTE);
}

pub fn build(ui: &mut Ui, d: &mut PropertiesDraft) {
    form::row(ui, "Build folder", |ui| {
        field::path(
            ui,
            &mut d.build_dir,
            "Build folder",
            "Choose build output folder",
            Browse::Folder,
            "",
        );
    });
    form::row(ui, "Executable name", |ui| {
        if d.adding {
            field::text(ui, &mut d.output_name, "", "Executable name");
        } else {
            field::fixed(ui, &d.output_name, "Executable name");
        }
    });
    form::row(ui, "Compiler workers", |ui| {
        let shown = if d.workers == 0 {
            w::USE_DEFAULT.to_string()
        } else {
            d.workers.to_string()
        };
        ui.add(
            egui::DragValue::new(&mut d.workers)
                .range(0..=64)
                .speed(0.1)
                .custom_formatter(move |_, _| shown.clone()),
        );
    });
    form::row(ui, "Engine", |ui| {
        ui.label(if d.game.nxrs() {
            w::ENGINE_NXRS
        } else {
            w::ENGINE_TURBO
        });
    });
}

pub fn runtime(ui: &mut Ui, app: &App, d: &mut PropertiesDraft) {
    let p = theme::of(ui.ctx());
    form::value(ui, "Graphics backend", w::VULKAN);
    let default = if app.lib.button_layout == "xbox" {
        w::LAYOUT_DEFAULT_XBOX
    } else {
        w::LAYOUT_DEFAULT_SWITCH
    };
    let layouts = select::words(&[
        (default, String::new()),
        (w::LAYOUT_SWITCH, "switch".into()),
        (w::LAYOUT_XBOX, "xbox".into()),
    ]);
    form::row(ui, "Controller layout", |ui| {
        select::select(ui, "Controller layout", &mut d.button_layout, &layouts);
    });
    let screen_default = if app.lib.docked {
        w::SCREEN_DEFAULT_TV
    } else {
        w::SCREEN_DEFAULT_HANDHELD
    };
    let modes = select::words(&[
        (screen_default, String::new()),
        (w::SCREEN_TV, "tv".into()),
        (w::SCREEN_HANDHELD, "handheld".into()),
    ]);
    form::row(ui, "Screen mode", |ui| {
        ui.scope(|ui| select::select(ui, "Screen mode", &mut d.screen_mode, &modes))
            .response
            .on_hover_text(w::SCREEN_TIP);
    });
    form::note(ui, &p, w::RUNTIME_NOTE);
    ui.add_space(6.0);
    let ini = d.game.ini_path();
    form::row(ui, "", |ui| {
        if ui
            .add_enabled(app.exists(&ini), egui::Button::new(w::OPEN_INI))
            .clicked()
        {
            app.open_path(&ini);
        }
    });
}

pub fn artwork(ui: &mut Ui, d: &mut PropertiesDraft) {
    let p = theme::of(ui.ctx());
    let images: (&str, &[&str]) = ("Images", &["png", "jpg", "jpeg", "webp", "bmp"]);
    form::row(ui, "Cover image", |ui| {
        field::path(
            ui,
            &mut d.cover,
            "Cover image",
            "Choose cover",
            Browse::File(Some(images)),
            "",
        );
    });
    form::row(ui, "Background image", |ui| {
        field::path(
            ui,
            &mut d.background,
            "Background image",
            "Choose background artwork",
            Browse::File(Some(images)),
            "",
        );
    });
    form::note(ui, &p, w::ARTWORK_NOTE);
}

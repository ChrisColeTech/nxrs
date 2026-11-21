//! Properties' General, Build, Runtime and Artwork sections (`editGame`) in the family's page grammar:
//! Groups of labelled rows, the explanations in the info marks' tooltips.

use crate::constants::{words_library as l, words_settings as w};
use crate::flows::App;
use crate::flows::properties::PropertiesDraft;
use crate::ui::widgets::field::{self, Browse};
use crate::ui::widgets::page::{group, readout, row, rule};
use crate::ui::widgets::rail::Icon;
use crate::ui::widgets::select;
use egui::Ui;

const NAME_TIP: &str = "The name the library shows. The build keeps its executable name.";
const SOURCE_TIP: &str = "The game package (NSP, XCI, NUP, an NRO) or an extracted folder (exefs/ + romfs/). Its data is read in place.";
const FOLDER_TIP: &str =
    "Where this game's build goes: the executable, the compiled code and the caches.";
const EXE_TIP: &str = "The build's file name; it is set when the game is added.";
const WORKERS_TIP: &str =
    "Compiler processes for this game's builds; Use default follows Settings > Build defaults.";
const LAYOUT_TIP: &str =
    "This game's controller layout; Use default follows Settings > Controller.";
const IMAGES: (&str, &[&str]) = ("Images", &["png", "jpg", "jpeg", "webp", "bmp"]);

pub fn general(ui: &mut Ui, app: &App, d: &mut PropertiesDraft) {
    group(ui, Icon::General, "Game", w::PATHS_NOTE);
    row(ui, "Display name", NAME_TIP, |ui| {
        field::text(ui, &mut d.title, "", "Display name");
    });
    ui.add_enabled_ui(!d.locked, |ui| {
        row(ui, "Source", SOURCE_TIP, |ui| {
            ui.vertical(|ui| {
                let filter = Browse::File(Some(l::GAME_FILTER));
                field::path(ui, &mut d.source, "Source", "Choose source", filter, "");
                if crate::ui::widgets::button::ghost(ui, w::CHOOSE_FOLDER, true).clicked()
                    && let Some(folder) = crate::flows::pick(
                        "Choose extracted game folder",
                        &d.source,
                        Browse::Folder,
                    )
                {
                    d.source = app.native(&folder);
                }
            });
        });
    });
    rule(ui);
    group(ui, Icon::General, "About", "");
    row(ui, "Format", "", |ui| readout(ui, d.game.format()));
    let version = app.version_of(&d.game);
    if !version.is_empty() {
        row(ui, "Version", "", |ui| readout(ui, &version));
    }
    row(ui, "Status", "", |ui| readout(ui, &d.status));
}

pub fn build(ui: &mut Ui, d: &mut PropertiesDraft) {
    group(ui, Icon::Build, "Build", FOLDER_TIP);
    row(ui, "Build folder", FOLDER_TIP, |ui| {
        let pick = "Choose build output folder";
        field::path(
            ui,
            &mut d.build_dir,
            "Build folder",
            pick,
            Browse::Folder,
            "",
        );
    });
    row(ui, "Executable name", EXE_TIP, |ui| {
        if d.adding {
            field::text(ui, &mut d.output_name, "", "Executable name");
        } else {
            field::fixed(ui, &d.output_name, "Executable name");
        }
    });
    row(ui, "Compiler workers", WORKERS_TIP, |ui| {
        let shown = if d.workers == 0 {
            w::USE_DEFAULT.to_string()
        } else {
            d.workers.to_string()
        };
        let value = egui::DragValue::new(&mut d.workers)
            .range(0..=64)
            .speed(0.1);
        ui.add(value.custom_formatter(move |_, _| shown.clone()));
    });
    let engine = if d.game.nxrs() {
        w::ENGINE_NXRS
    } else {
        w::ENGINE_TURBO
    };
    row(ui, "Engine", "", |ui| readout(ui, engine));
}

pub fn runtime(ui: &mut Ui, app: &App, d: &mut PropertiesDraft) {
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
    group(ui, Icon::Runtime, "Runtime", w::RUNTIME_NOTE);
    row(ui, "Screen mode", w::SCREEN_TIP, |ui| {
        select::select(ui, "Screen mode", &mut d.screen_mode, &modes);
    });
    row(ui, "Controller layout", LAYOUT_TIP, |ui| {
        select::select(ui, "Controller layout", &mut d.button_layout, &layouts);
    });
    row(ui, "Graphics backend", "", |ui| readout(ui, w::VULKAN));
    let ini = d.game.ini_path();
    row(ui, "Settings file", w::RUNTIME_NOTE, |ui| {
        if crate::ui::widgets::button::ghost(ui, w::OPEN_INI, app.exists(&ini)).clicked() {
            app.open_path(&ini);
        }
    });
}

pub fn artwork(ui: &mut Ui, d: &mut PropertiesDraft) {
    group(ui, Icon::Artwork, "Artwork", w::ARTWORK_NOTE);
    row(
        ui,
        "Cover image",
        "The card's picture in the gallery.",
        |ui| {
            let browse = Browse::File(Some(IMAGES));
            field::path(ui, &mut d.cover, "Cover image", "Choose cover", browse, "");
        },
    );
    row(ui, "Background image", w::ARTWORK_NOTE, |ui| {
        let browse = Browse::File(Some(IMAGES));
        let pick = "Choose background artwork";
        field::path(ui, &mut d.background, "Background image", pick, browse, "");
    });
}

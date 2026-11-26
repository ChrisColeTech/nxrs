//! The windowed run: the main window (1280 x 800, at least 920 x 600, where it was last), the fonts, the
//! title bar taken over on Windows, the flags' views and windows, `--screenshot`, `--show-menu`.

use super::{Options, startup};
use crate::constants::{layout, timing};
use crate::flows::App;
use crate::ui::shell::Shell;
use crate::ui::{screenshot, title_bar_install};
use std::sync::Arc;

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../../assets/app.png"))
        .ok()?
        .to_rgba8();
    Some(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

fn viewport(app: &App) -> egui::ViewportBuilder {
    let mut v = egui::ViewportBuilder::default()
        .with_title(layout::WINDOW_TITLE)
        .with_inner_size(layout::WINDOW_SIZE)
        .with_min_inner_size(layout::WINDOW_MIN)
        .with_decorations(cfg!(windows))
        .with_drag_and_drop(true);
    if let Some(w) = &app.lib.window {
        v = v
            .with_inner_size([
                w.width.max(layout::WINDOW_MIN[0]),
                w.height.max(layout::WINDOW_MIN[1]),
            ])
            .with_position([w.x, w.y])
            .with_maximized(w.maximized);
    }
    if let Some(i) = icon() {
        v = v.with_icon(Arc::new(i));
    }
    v
}

pub fn run(path: &str, o: &Options) -> i32 {
    crate::services::startup_log::note(&format!("library {path}"));
    let mut probe = App::new(path, o.demo, crate::services::jobs::noop());
    probe.open();
    crate::services::startup_log::note(&format!("library open: {} games", probe.lib.games.len()));
    // build folders in the layout before move to theirs (a preview picture changes nothing)
    if o.screenshot.is_none() {
        for line in probe.migrate_build_folders(None).lines {
            crate::services::startup_log::note(&line);
        }
    }
    let builder = viewport(&probe);
    let native = eframe::NativeOptions {
        viewport: builder,
        persist_window: false,
        ..Default::default()
    };
    let shot = Arc::new(std::sync::Mutex::new(1));
    let exit = shot.clone();
    let opts = o.clone();
    crate::services::startup_log::note("opening the window");
    let started = eframe::run_native(
        layout::WINDOW_TITLE,
        native,
        Box::new(move |cc| {
            crate::services::startup_log::note("window created");
            let ctx = cc.egui_ctx.clone();
            let notify: crate::services::jobs::Notify = Arc::new(move || ctx.request_repaint());
            let mut app = probe;
            app.notify = notify.clone();
            title_bar_install::install(cc, notify.clone(), app.appearance.0 != "light");
            if let Err(code) = startup::apply(&mut app, &opts) {
                if let Ok(mut e) = exit.lock() {
                    *e = code;
                }
                cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            app.refresh_tools();
            startup::windows(&mut app, &opts);
            let mut shell = Shell::new(app, notify);
            if let Some(png) = &opts.screenshot {
                shell.shot.take = Some((png.clone(), opts.screenshot_delay as f64 / 1000.0));
                // a dialog is drawn inside the main window for the picture, as the Qt launcher composited it
                cc.egui_ctx.set_embed_viewports(true);
            }
            if let Some(menu) = &opts.show_menu {
                shell.menu_at = Some((menu.clone(), timing::SHOW_MENU_MS as f64 / 1000.0));
            }
            Ok(Box::new(ShotExit { shell, exit }))
        }),
    );
    match (started, o.screenshot.is_some(), shot.lock()) {
        (Err(e), ..) => {
            crate::services::platform::fatal(&format!("The window could not open: {e}"));
            1
        }
        (Ok(()), true, Ok(code)) => *code,
        (Ok(()), false, Ok(code)) if *code == 2 => 2,
        _ => 0,
    }
}

/// The shell, and the exit code `--screenshot` ends with.
struct ShotExit {
    shell: Shell,
    exit: Arc<std::sync::Mutex<i32>>,
}
impl eframe::App for ShotExit {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.shell.logic(ctx, frame);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.shell.ui(ui, frame);
        if self.shell.shot.asked
            && let Ok(mut e) = self.exit.lock()
            && *e != 2
        {
            *e = screenshot::exit_code(&self.shell.shot);
        }
    }
}

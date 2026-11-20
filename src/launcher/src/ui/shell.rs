//! The main window (the Qt `Window`'s layout): title bar, Commands strip, the library over its canvas,
//! the build output panel, the status bar; the dialogs in their child windows; shortcuts, drops, the
//! close protocol, `--screenshot`.

use super::chrome::{caption, commands, filter_popover, status_bar, title_bar, tool_popup};
use super::library::artwork_cache::ArtworkCache;
use super::popups::{child_window, held};
use super::{keys, screenshot, theme, windows};
use crate::constants::layout;
use crate::flows::surface::Surface;
use crate::flows::{Action, App};
use egui::{CentralPanel, Frame, Panel, Ui};

pub struct Shell {
    pub app: App,
    pub bar: title_bar::BarState,
    pub add_open: bool,
    pub art: ArtworkCache,
    pub applied: (String, String),
    pub shot: screenshot::Shot,
    pub menu_at: Option<(String, f64)>,
    /// the system fonts are installed (they take effect from the next frame)
    pub fonts: bool,
}

impl Shell {
    pub fn new(app: App, notify: crate::services::jobs::Notify) -> Shell {
        Shell {
            app,
            bar: title_bar::BarState::default(),
            add_open: false,
            art: ArtworkCache::new(notify),
            applied: (String::new(), String::new()),
            shot: screenshot::Shot::default(),
            menu_at: None,
            fonts: false,
        }
    }

    /// One frame of the main window (eframe's `logic` and `ui` together; the test harness calls this).
    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.app.tick();
        self.art.invalidate(self.app.artwork_epoch);
        self.art.upload(&ctx);
        self.draw(ui);
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied != self.app.appearance {
            self.applied = self.app.appearance.clone();
            theme::apply(ctx, &theme::palette(&self.applied.0, &self.applied.1));
            #[cfg(windows)]
            crate::services::platform::windows_caption::set_dark(self.applied.0 != "light");
        }
    }

    fn layout(&mut self, ui: &mut Ui) -> Vec<Action> {
        let mut actions = Vec::new();
        let ctx = ui.ctx().clone();
        let bar_height = layout::BAR_HEIGHT + caption::top_inset(ctx.pixels_per_point());
        child_window::note_bar(&ctx, bar_height);
        Panel::top("title-bar")
            .exact_size(bar_height)
            .frame(Frame::NONE)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                actions.extend(title_bar::show(ui, &self.app, &mut self.bar));
            });
        let mut anchor = egui::Rect::NOTHING;
        Panel::top("commands")
            .exact_size(layout::COMMANDS_HEIGHT)
            .frame(Frame::NONE)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                let (a, r) = commands::show(ui, &self.app, &mut self.add_open);
                actions.extend(a);
                anchor = r;
            });
        let mut badge = egui::Rect::NOTHING;
        Panel::bottom("status-bar")
            .exact_size(layout::STATUS_HEIGHT)
            .frame(Frame::NONE)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                let (a, r) = status_bar::show(ui, &self.app);
                actions.extend(a);
                badge = r;
            });
        actions.extend(self.console_window(ui));
        CentralPanel::no_frame().show(ui, |ui| actions.extend(self.body(ui)));
        if self.app.filters.open {
            actions.extend(filter_popover::show(&ctx, &mut self.app, anchor));
        }
        if matches!(self.app.surfaces.last(), Some(Surface::ToolPopup)) {
            let (keep, a) = tool_popup::show(&ctx, &self.app, badge);
            if !keep {
                self.app.surfaces.pop();
            }
            actions.extend(a);
        }
        actions
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.tick();
        self.art.invalidate(self.app.artwork_epoch);
        self.art.upload(ctx);
        if self.app.working() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(
            crate::constants::timing::POLL_MS,
        ));
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl Shell {
    fn draw(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // the fonts the theme names (semibold, mono) exist from the next frame on
        if !std::mem::replace(&mut self.fonts, true) {
            ctx.set_fonts(super::fonts::definitions(
                crate::services::font_files::load(),
            ));
            ctx.request_repaint();
            return;
        }
        self.apply_theme(&ctx);
        caption::edge_resize(&ctx);
        let modal = self
            .app
            .surfaces
            .iter()
            .any(|s| !matches!(s, Surface::ToolPopup));
        let mut actions = if modal {
            Vec::new()
        } else {
            keys::shortcuts(&ctx, &self.app)
        };
        let laid = self.layout(ui);
        if !modal {
            actions.extend(laid);
        }
        actions.extend(windows::show(&ctx, &mut self.app));
        if let Some((name, at)) = self.menu_at.take() {
            if ctx.input(|i| i.time) >= at {
                title_bar::open_menu(&mut self.bar, &name)
            } else {
                self.menu_at = Some((name, at))
            }
        }
        for action in actions {
            self.app.dispatch(action);
        }
        screenshot::step(&ctx, &mut self.shot);
        held::note(&ctx);
        self.close_protocol(&ctx);
    }
}

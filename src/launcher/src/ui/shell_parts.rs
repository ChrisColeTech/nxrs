//! The main window's library area (canvas, gallery or list, the empty text, files dropped on it)
//! and its close protocol (a build running asks first; the placement is kept).

use super::chrome::{console, dock};
use super::library::{self, canvas, gallery, list};
use super::popups::child_window;
use super::screenshot;
use super::shell::Shell;
use crate::flows::Action;
use egui::{Ui, ViewportCommand};

impl Shell {
    /// The build output popped out: a tool window beside the main one, which keeps working. Docked, it
    /// is drawn in `body`.
    pub(super) fn console_window(&mut self, ui: &mut Ui) -> Vec<Action> {
        let ctx = ui.ctx().clone();
        let c = self.app.lib.console.clone();
        if !(c.visible && c.floating) {
            child_window::shut(&ctx, "console");
        }
        if c.visible && c.floating {
            let title = crate::constants::words_build::OUTPUT;
            let spec =
                child_window::ChildSpec::tool("console", title, 720.0, 360.0).least(360.0, 160.0);
            let mut action = None;
            let closed =
                child_window::show(&ctx, &spec, |ui| action = console::show(ui, &mut self.app));
            if closed {
                self.app.lib.console.visible = false;
            }
            return action.into_iter().collect();
        }
        Vec::new()
    }

    /// The library and, docked, the build output on the geometry `dock` owns.
    pub(super) fn body(&mut self, ui: &mut Ui) -> Vec<Action> {
        let area = ui.max_rect();
        let c = self.app.lib.console.clone();
        let s = dock::split(area, dock::painted(&c, dock::shown(ui, &c)), c.right);
        let mut actions = self.library(&mut dock::child(ui, s.library, "library"));
        if s.console.width() > 1.0 && s.console.height() > 1.0 {
            // the border is inside the build output's own box: its content gives up 1px
            let inner = if c.right {
                s.console.with_min_x(s.console.left() + 1.0)
            } else {
                s.console.with_min_y(s.console.top() + 1.0)
            };
            actions.extend(console::show(
                &mut dock::child(ui, inner, "console"),
                &mut self.app,
            ));
            if c.visible && !c.floating {
                dock::sash_and_rule(ui, &mut self.app.lib.console, &s);
            }
        }
        actions
    }

    pub(super) fn library(&mut self, ui: &mut Ui) -> Vec<Action> {
        let rect = ui.max_rect();
        canvas::show(ui, &self.app, &mut self.art, rect);
        let games = self.app.visible_games();
        let mut actions = Vec::new();
        if games.is_empty() {
            library::empty(ui, &self.app, rect);
        } else if self.app.lib.view_mode == 1 {
            actions.extend(list::show(ui, &mut self.app, &games));
        } else {
            actions.extend(gallery::show(ui, &mut self.app, &mut self.art, &games));
        }
        let drops: Vec<String> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| crate::model::path_text::from_native(&f.path().to_string_lossy()))
                .collect()
        });
        if !drops.is_empty() && self.app.surfaces.is_empty() {
            self.app.drop_files(&drops);
        }
        actions
    }

    pub(super) fn close_protocol(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.app.close_now {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.app.request_close();
        }
        if self.app.close_now {
            screenshot::keep_placement(ctx, &mut self.app.lib);
            self.app.save();
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

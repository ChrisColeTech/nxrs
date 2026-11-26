//! Settings (`showSettings`): a Draft of the launcher's settings. Appearance previews live and Cancel
//! restores it; Apply checks a build is not running for the engine and build defaults, that a chosen
//! nxrs-build exists, then writes a changed layout or Play setting into every built game.

use super::App;
use super::surface::Surface;
use crate::constants::{engine as e, words_settings as w};
use crate::model::fill::fill;
use crate::model::path_text::{absolute, from_native, native, same_path};
use crate::services::{build_facts, files, ini_file};

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsDraft {
    pub tab: usize,
    pub nxrs_build: String,
    pub output_root: String,
    pub jobs: i64,
    pub button_layout: String,
    pub color_mode: String,
    pub accent: String,
    pub show_background: bool,
    pub docked: bool,
    pub pause: bool,
    pub mute: bool,
    /// what Build defaults' Move said in this window (the move is done at once, not on Apply)
    pub moved: Vec<String>,
}

impl App {
    pub fn show_settings(&mut self, tab: usize) {
        let l = &self.lib;
        let draft = SettingsDraft {
            tab: tab.min(4),
            // the engine in use: the one chosen, else the one found (the bundled one)
            nxrs_build: native(if l.nxrs_build.is_empty() {
                &self.tools.nxrs_build
            } else {
                &l.nxrs_build
            }),
            output_root: l.output_root.clone(),
            jobs: l.jobs,
            button_layout: l.button_layout.clone(),
            color_mode: self.appearance.0.clone(),
            accent: self.appearance.1.clone(),
            show_background: l.show_background,
            docked: l.docked,
            pause: l.pause_on_focus_loss,
            mute: l.mute_on_focus_loss,
            moved: Vec::new(),
        };
        self.surfaces.push(Surface::Settings(Box::new(draft)));
        self.run_doctor();
    }

    /// Cancel, Escape or the X: the appearance as it was saved.
    pub fn cancel_settings(&mut self) {
        self.appearance = (self.lib.color_mode.clone(), self.lib.accent_color.clone());
    }

    /// Apply: true when the window closes.
    pub fn apply_settings(&mut self, s: &SettingsDraft) -> bool {
        let chosen = from_native(s.nxrs_build.trim());
        // the found engine left as shown is no choice: it stays found (a moved release finds its own)
        let found = self.lib.nxrs_build.is_empty() && same_path(&chosen, &self.tools.nxrs_build);
        let chosen = if found { String::new() } else { chosen };
        let engine_changed = s.output_root != self.lib.output_root
            || s.jobs != self.lib.jobs
            || chosen != self.lib.nxrs_build;
        if self.builder_busy() && engine_changed {
            self.message(w::BUSY_TITLE, w::BUSY, false);
            return false;
        }
        if !chosen.is_empty() && chosen != self.lib.nxrs_build && !files::exists(&chosen) {
            self.message(
                w::NXRS_MISSING_TITLE,
                &fill(w::NXRS_MISSING, &[e::EXECUTABLE_SUFFIX]),
                true,
            );
            return false;
        }
        let l = &mut self.lib;
        l.nxrs_build = if chosen.is_empty() {
            String::new()
        } else {
            absolute(&chosen)
        };
        (l.output_root, l.jobs, l.show_background) = (
            from_native(s.output_root.trim()),
            s.jobs.clamp(1, 64),
            s.show_background,
        );
        (l.color_mode, l.accent_color) = (s.color_mode.clone(), s.accent.clone());
        if s.button_layout != self.lib.button_layout {
            self.lib.button_layout = s.button_layout.clone();
            let (failed, error) = ini_file::write_all(&self.lib.games, |g| {
                (g.button_layout.is_empty() && build_facts::has_build(g))
                    .then(|| ini_file::write_button_layout(&g.ini_path(), &s.button_layout))
            });
            if failed > 0 {
                self.message(
                    crate::constants::words_play::NO_SETTINGS,
                    &fill(w::LAYOUT_FAILED, &[&failed.to_string(), &error]),
                    true,
                );
            }
        }
        let l = &self.lib;
        if (s.pause, s.mute, s.docked) != (l.pause_on_focus_loss, l.mute_on_focus_loss, l.docked) {
            let l = &mut self.lib;
            (l.pause_on_focus_loss, l.mute_on_focus_loss, l.docked) = (s.pause, s.mute, s.docked);
            let lib = self.lib.clone();
            let (failed, error) = ini_file::write_all(&lib.games, |g| {
                build_facts::has_build(g)
                    .then(|| ini_file::write_play_settings(&g.ini_path(), &lib.play_for(g)))
            });
            if failed > 0 {
                self.message(
                    crate::constants::words_play::NO_SETTINGS,
                    &fill(w::PLAY_FAILED, &[&failed.to_string(), &error]),
                    true,
                );
            }
        }
        self.appearance = (s.color_mode.clone(), s.accent.clone());
        self.save();
        self.refresh_tools();
        true
    }
}

//! Play and Stop (`Window::play`, `Window::stop`): the guards, the DLC sync first when needed, the
//! INI settings, the detached start.

use super::{App, DlcReason};
use crate::constants::{words_contexts as c, words_play as w};
use crate::model::dates;
use crate::services::{dlc_files, ini_file, platform};

impl App {
    pub fn play(&mut self) {
        let Some(id) = self.selected_id() else { return };
        let f = self.facts_of(&id);
        if self.demo || !f.playable() || self.busy(&id) {
            return;
        }
        if f.source_offline() {
            self.message(c::PLAY, w::OFFLINE_TITLE, w::OFFLINE, true);
            return;
        }
        if self.dlc_busy() {
            return;
        }
        // add-on content: the data folder holds the game's DLC list before the game starts
        let adopted = self
            .lib
            .find_mut(&id)
            .is_some_and(dlc_files::adopt_installed);
        if adopted {
            self.save();
        }
        if !self.lib.find(&id).is_some_and(dlc_files::up_to_date) {
            self.start_dlc_sync(&id, DlcReason::Play);
            return;
        }
        self.launch();
    }

    /// Writes the game's settings and starts it detached; records its pid and when it was played.
    pub(super) fn launch(&mut self) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        if let Err(err) =
            ini_file::write_game_settings(&game, &self.lib.button_layout, &self.lib.play())
        {
            self.message(c::PLAY, w::NO_SETTINGS, &err, true);
            return;
        }
        match platform::launch_detached(
            &game.executable(),
            &game.build_dir,
            &self.tools.environment(),
        ) {
            Err(err) => self.message(c::PLAY, w::NOT_LAUNCHED, &err, true),
            Ok(pid) => {
                self.running.insert(game.id.clone(), pid);
                if let Some(g) = self.lib.find_mut(&game.id) {
                    g.last_played = Some(dates::now());
                }
                self.save();
                self.job_label = w::LAUNCHED.into();
            }
        }
    }

    /// Cancels the selected game's build, or asks the running game to close (never forces).
    pub fn stop(&mut self) {
        let Some(id) = self.selected_id() else { return };
        if self.is_building(&id) {
            self.cancel_build();
            return;
        }
        let Some(pid) = self.running.get(&id).copied() else {
            return;
        };
        let exe = self
            .lib
            .find(&id)
            .map(|g| g.executable())
            .unwrap_or_default();
        if platform::request_close(pid, &exe) {
            self.job_label = w::CLOSE_ASKED.into();
        } else {
            self.message(c::STOP, w::NO_CLOSE_TITLE, w::NO_CLOSE, false);
        }
    }
}

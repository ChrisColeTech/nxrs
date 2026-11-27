//! The one place every command is handled.

use super::surface::Surface;
use super::{Action, App};
use crate::constants::words_library as w;
use crate::model::path_text::parent;
use crate::services::{files, shell_open};

impl App {
    pub fn dispatch(&mut self, action: Action) {
        match action {
            Action::AddGame => self.pick_games(),
            Action::AddFolder => self.pick_folder(),
            Action::ImportBuildPick => self.pick_build(),
            Action::Settings(tab) => self.show_settings(tab),
            Action::Exit => self.request_close(),
            Action::Play => self.play(),
            Action::Stop => self.stop(),
            Action::Build => self.start_build(),
            Action::BuildWeb => self.start_web_build(),
            Action::PlayWeb => self.play_in_browser(),
            Action::Properties(tab) => self.show_properties(tab),
            Action::InstallDlc => self.show_dlc(),
            Action::InstallUpdates => self.show_updates(),
            Action::RemoveGame => self.remove_game(),
            Action::InstallKeys => self.install_keys(),
            Action::OpenKeysFolder => {
                let dir = parent(&self.lib.keys_path);
                if !self.lib.keys_path.is_empty() && files::is_dir(&dir) {
                    shell_open::open(&dir);
                }
            }
            Action::ViewMode(m) => {
                self.lib.view_mode = i64::from(m == 1);
                self.save();
            }
            Action::ToggleFilters => {
                self.filters.open = !self.filters.open;
                self.filters.focus_search = self.filters.open;
            }
            Action::ToggleBackground => {
                self.lib.show_background = !self.lib.show_background;
                self.save();
            }
            Action::Refresh => {
                self.artwork_epoch += 1;
                self.refresh_tools();
            }
            Action::ViewConsole => self.lib.console.visible = true,
            Action::ToggleConsole => self.lib.console.visible = !self.lib.console.visible,
            Action::About => self.message("nxrs", w::ABOUT, false),
            Action::OpenBuildFolder => self.open_folder(false),
            Action::OpenSaveFolder => self.open_folder(true),
            Action::ViewGameLog => self.open_game_log(),
            Action::OpenBuildLog => self.open_build_log(),
            Action::ClearConsole => self.console.clear(),
            Action::ToolPopup => self.toggle_tool_popup(),
            Action::Select(id) => self.select(&id),
            Action::Activate => self.activate(),
            Action::CardPrimary(id) => self.card_action(&id),
            Action::CardProperties(id) => {
                self.select(&id);
                self.show_properties(0);
            }
            Action::CardActivate(id) => self.card_activate(&id),
            Action::ConfirmRemove(id, delete) => self.confirm_remove(&id, delete),
            Action::ConfirmClose => self.close_after_cancel(),
        }
        self.refresh_facts();
        self.fix_selection();
    }

    fn toggle_tool_popup(&mut self) {
        if matches!(self.surfaces.last(), Some(Surface::ToolPopup)) {
            self.surfaces.pop();
        } else {
            self.surfaces.push(Surface::ToolPopup);
        }
    }

    /// The card's cover double-clicked: Play when it offers Play, else Properties > General.
    fn card_activate(&mut self, id: &str) {
        self.select(id);
        let plays = self
            .lib
            .find(id)
            .is_some_and(|g| self.card_of(g).action == crate::constants::menus::PLAY);
        if plays {
            self.card_action(id);
        } else {
            self.show_properties(0);
        }
    }
}

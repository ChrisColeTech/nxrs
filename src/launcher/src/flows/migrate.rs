//! The build-folder migration as a command (`--migrate-builds`, and once when the window opens): the
//! games running or building now are left; the library file is copied aside before the first write.

use super::App;
use crate::constants::engine as e;
use crate::model::build_layout::migrated_build_dir;
use crate::model::path_text::{join, parent};
use crate::services::build_move::{self, Migration};
use crate::services::{files, platform};
use std::collections::BTreeSet;

impl App {
    /// `only`: the title of the one entry to move (`--select-title`); None: every entry.
    pub fn migrate_build_folders(&mut self, only: Option<&str>) -> Migration {
        let pending = self.lib.games.iter().any(|g| {
            migrated_build_dir(&g.build_dir).is_some() && only.is_none_or(|t| t == g.title)
        });
        if self.demo || !pending || !self.store.writable {
            return Migration::default();
        }
        let backup = join(&parent(&self.lib.path), e::PRE_LAYOUT_BACKUP);
        if files::is_file(&self.lib.path)
            && !files::exists(&backup)
            && let Err(err) = files::copy_file(&self.lib.path, &backup)
        {
            return Migration {
                lines: vec![err],
                skipped: 1,
                ..Migration::default()
            };
        }
        let mut busy: BTreeSet<String> = platform::find_running_games(&self.lib.games)
            .into_keys()
            .collect();
        busy.extend(self.running.keys().cloned());
        busy.extend(self.build.as_ref().map(|b| b.game.id.clone()));
        let m = build_move::migrate(&mut self.lib, &busy, only);
        if m.changed {
            self.save();
        }
        self.refresh_facts();
        m
    }
}

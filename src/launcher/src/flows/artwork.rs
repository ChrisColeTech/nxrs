//! The launcher owns its artwork (owner, 2026-11-27): whatever sets a cover or background (Add game,
//! Properties, Import build) copies the image into the artwork folder and the entry points there; at
//! the window's start every entry's outside artwork is copied in once (`own_all_artwork`).

use super::App;
use crate::constants::{engine as e, words_import as w};
use crate::model::fill::fill;
use crate::model::path_text::{join, native, parent};
use crate::services::{artwork_store, files};

impl App {
    /// The artwork folder beside the library file.
    pub fn artwork_folder(&self) -> String {
        artwork_store::folder(&self.lib.path)
    }

    /// Copies the entry's artwork in (an image that cannot be copied stays as it is, logged); whether
    /// the entry changed. The caller saves.
    pub(super) fn own_artwork(&mut self, id: &str) -> bool {
        if self.demo {
            return false;
        }
        let dir = self.artwork_folder();
        let Some(g) = self.lib.find_mut(id) else {
            return false;
        };
        let owned = artwork_store::own(&dir, g);
        owned.left.iter().for_each(|l| self.console.push(l));
        if owned.changed {
            self.artwork_epoch += 1;
        }
        owned.changed
    }

    /// Every entry's outside artwork copied in and the entries repointed; idempotent (art already in
    /// the folder is left; a missing image is left as it is and logged). The library file is copied
    /// to `library.pre-artwork.json` before the first write. Returns the lines said.
    pub fn own_all_artwork(&mut self) -> Vec<String> {
        if self.demo || !self.store.writable {
            return Vec::new();
        }
        let dir = self.artwork_folder();
        let mut lines = Vec::new();
        let mut copied = 0usize;
        let mut games = self.lib.games.clone();
        for g in &mut games {
            let before = (g.cover.clone(), g.background.clone());
            let owned = artwork_store::own(&dir, g);
            copied += usize::from(before.0 != g.cover) + usize::from(before.1 != g.background);
            lines.extend(owned.left);
        }
        if copied > 0 {
            let backup = join(&parent(&self.lib.path), e::PRE_ARTWORK_BACKUP);
            let backed_up = !files::is_file(&self.lib.path)
                || files::exists(&backup)
                || files::copy_file(&self.lib.path, &backup).is_ok();
            if backed_up {
                self.lib.games = games;
                self.save();
                self.artwork_epoch += 1;
                lines.push(fill(w::ART_ADOPTED, &[&copied.to_string(), &native(&dir)]));
            } else {
                // never repointed without the backup: the entries stay as they were until a start that can
                lines.push(fill(w::BACKUP_FAILED, &[&native(&backup)]));
            }
        }
        lines.iter().for_each(|l| self.console.push(l));
        lines
    }
}

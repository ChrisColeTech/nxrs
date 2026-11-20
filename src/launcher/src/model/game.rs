//! A library entry and what follows from it without looking at the disk: its files' names, its format,
//! the update the next build applies.

use super::game_update::GameUpdate;
use super::path_text::{ends_with_ci, join, same_path};
use crate::constants::engine as e;
use chrono::NaiveDateTime;
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub source: String,
    pub build_dir: String,
    pub output_name: String,
    pub cover: String,
    pub background: String,
    pub renderer: String,
    /// "nxrs", or "turbo" for an entry the Turbo engine built (it needs an nxrs build now)
    pub engine: String,
    pub button_layout: String,
    pub screen_mode: String,
    pub built_source: String,
    pub dlc: Vec<String>,
    pub updates: Vec<GameUpdate>,
    /// "" newest, "none" without updates, else a package path
    pub update_choice: String,
    pub built_update: String,
    pub build_incomplete: bool,
    pub workers: i64,
    pub optimization: i64,
    pub last_played: Option<NaiveDateTime>,
    pub last_built: Option<NaiveDateTime>,
    /// kept in the file as it was (no longer shown)
    pub hints_built: i64,
    pub saves_offered: String,
    /// keys this launcher does not know, written back unchanged
    pub extra: Map<String, Value>,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            id: String::new(),
            title: String::new(),
            source: String::new(),
            build_dir: String::new(),
            output_name: String::new(),
            cover: String::new(),
            background: String::new(),
            renderer: "vulkan".into(),
            engine: e::ENGINE_TURBO.into(),
            button_layout: String::new(),
            screen_mode: String::new(),
            built_source: String::new(),
            dlc: Vec::new(),
            updates: Vec::new(),
            update_choice: String::new(),
            built_update: String::new(),
            build_incomplete: false,
            workers: 0,
            optimization: 2,
            last_played: None,
            last_built: None,
            hints_built: 0,
            saves_offered: String::new(),
            extra: Map::new(),
        }
    }
}

impl Game {
    pub fn nxrs(&self) -> bool {
        self.engine == e::ENGINE_NXRS
    }
    /// An entry built with the Turbo engine: it plays no more and needs an nxrs build.
    pub fn turbo(&self) -> bool {
        !self.nxrs()
    }
    pub fn executable(&self) -> String {
        join(
            &self.build_dir,
            &format!("{}{}", self.output_name, e::EXECUTABLE_SUFFIX),
        )
    }
    pub fn code_module(&self) -> String {
        join(
            &self.build_dir,
            &format!("{}{}", self.output_name, e::MODULE_SUFFIX),
        )
    }
    pub fn data_dir(&self) -> String {
        join(&self.build_dir, &format!("{}_data", self.output_name))
    }
    pub fn ini_path(&self) -> String {
        join(&self.build_dir, &format!("{}.ini", self.output_name))
    }
    pub fn build_log(&self) -> String {
        join(&self.build_dir, &format!("{}.build.log", self.output_name))
    }
    pub fn format(&self) -> &'static str {
        let s = &self.source;
        if ends_with_ci(s, ".nup") {
            "DIRECT NUP"
        } else if ends_with_ci(s, ".nsp") {
            "DIRECT NSP"
        } else if ends_with_ci(s, ".xci") {
            "DIRECT XCI"
        } else if ends_with_ci(s, ".nro") {
            "HOMEBREW"
        } else if s.is_empty() {
            "NATIVE BUILD"
        } else {
            "EXTRACTED"
        }
    }
    /// The update the next build applies: the chosen one, else the newest; none for "none".
    pub fn active_update(&self) -> Option<&GameUpdate> {
        if self.update_choice == "none" {
            return None;
        }
        let chosen = self.update_choice.as_str();
        if !chosen.is_empty()
            && let Some(u) = self.updates.iter().find(|u| same_path(&u.path, chosen))
        {
            return Some(u);
        }
        self.updates
            .iter()
            .fold(None, |n: Option<&GameUpdate>, u| match n {
                Some(n) if n.version >= u.version => Some(n),
                _ => Some(u),
            })
    }
    /// What the next build applies: "" (the package as it is), "none" or the update's path.
    pub fn update_token(&self) -> String {
        if self.update_choice == "none" {
            return "none".into();
        }
        self.active_update()
            .map(|u| u.path.clone())
            .unwrap_or_default()
    }
}

/// Two update tokens name the same build: equal, or the same package path.
pub fn same_token(left: &str, right: &str) -> bool {
    left == right
        || (left != "none"
            && right != "none"
            && !left.is_empty()
            && !right.is_empty()
            && same_path(left, right))
}

/// An in-place package: the game is read from it while it plays (NUP, NSP, XCI).
pub fn in_place_package(path: &str) -> bool {
    ends_with_ci(path, ".nup") || ends_with_ci(path, ".nsp") || ends_with_ci(path, ".xci")
}

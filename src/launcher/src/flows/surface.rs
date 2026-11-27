//! The surfaces that open over the main window: the dialogs (each a modal child window), the message and
//! question boxes, the engine badge's popup.

use super::Action;
use super::properties::PropertiesDraft;
use super::settings::SettingsDraft;
use std::collections::BTreeSet;

/// A list's selection: rows chosen, and the row a Shift-click extends from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picks {
    pub rows: BTreeSet<usize>,
    pub anchor: Option<usize>,
}

/// Game > Install DLC… / Install Updates…: the window works on the library at once; it only keeps its
/// own selection and the last outcome.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListWindow {
    pub game_id: String,
    /// the game was running or building when the window opened: read-only
    pub locked: bool,
    pub picks: Picks,
    /// the last outcome, under the list (text, an error)
    pub status: Option<(String, bool)>,
}

/// A question's tick box: its words, its explanation (the info mark), whether it is ticked, and what Yes
/// asks for while it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub label: String,
    pub tip: String,
    pub on: bool,
    pub yes_on: Action,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Surface {
    Message {
        title: String,
        text: String,
        error: bool,
    },
    Question {
        title: String,
        text: String,
        yes: Action,
        no: Option<Action>,
        default_no: bool,
        check: Option<Check>,
    },
    Properties(Box<PropertiesDraft>),
    Settings(Box<SettingsDraft>),
    Dlc(ListWindow),
    Updates(ListWindow),
    /// Play or Build (`build`) asks where: Native or Web (owner, 2026-11-27: one Play, one Build)
    Target {
        game_id: String,
        build: bool,
    },
    ToolPopup,
}

impl Surface {
    /// The child window's key (one native window per key).
    pub fn key(&self, at: usize) -> String {
        match self {
            Surface::Message { .. } | Surface::Question { .. } => format!("message-{at}"),
            Surface::Properties(p) if p.adding => "add-game".into(),
            Surface::Properties(_) => "properties".into(),
            Surface::Settings(_) => "settings".into(),
            Surface::Dlc(_) => "dlc".into(),
            Surface::Updates(_) => "updates".into(),
            Surface::Target { .. } => "target".into(),
            Surface::ToolPopup => "tool-popup".into(),
        }
    }
}

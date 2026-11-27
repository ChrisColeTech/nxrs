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

/// A question's words: the window's title, the heading (the question itself), the text under it
/// (may be empty) and the verb of its one filled button ("Remove"); the X and Escape answer No.
/// `why_not`: why the verb cannot answer now (its disabled button's tooltip); empty: it can.
/// `lead`: a second verb, a ghost button at the footer's leading side (Play's and Build's Web).
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub title: String,
    pub heading: String,
    pub text: String,
    pub verb: String,
    pub why_not: String,
    pub lead: Option<Box<Lead>>,
}

/// A question's leading verb: its label, what it answers, why it cannot now (empty: it can).
#[derive(Clone, Debug, PartialEq)]
pub struct Lead {
    pub verb: String,
    pub action: Action,
    pub why_not: String,
}

impl Ask {
    pub fn new(title: &str, heading: &str, text: &str, verb: &str) -> Self {
        let own = |s: &str| s.to_string();
        Self {
            title: own(title),
            heading: own(heading),
            text: own(text),
            verb: own(verb),
            why_not: String::new(),
            lead: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Surface {
    /// A message box: `context` titles the window (the task or place it belongs to: "Add game",
    /// "Build"), `title` is its heading (the message itself: "Cannot add game").
    Message {
        context: String,
        title: String,
        text: String,
        error: bool,
    },
    Question {
        ask: Ask,
        yes: Action,
        no: Option<Action>,
        default_no: bool,
        check: Option<Check>,
    },
    Properties(Box<PropertiesDraft>),
    Settings(Box<SettingsDraft>),
    Dlc(ListWindow),
    Updates(ListWindow),
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
            Surface::ToolPopup => "tool-popup".into(),
        }
    }
}

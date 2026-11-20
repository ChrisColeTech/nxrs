//! What a child window is (`child_window`): its key, title, size, place, chrome and what Escape does.

use crate::constants::{child_window as c, popups as p};
use egui::{Vec2, vec2};

/// Where a window is put the first time it opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// beside the main window's corner (a tool left up while the main window is used)
    Corner,
    /// centred on the main window (the task at hand)
    Centre,
}

/// Whether the window has its title header and can be dismissed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chrome {
    Titled,
    Bare,
}

/// What Escape does in a window; a text field or an open menu has it first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Escape {
    Never,
    Always,
    UnlessTyping,
}

pub struct ChildSpec<'a> {
    pub key: &'a str,
    pub title: &'a str,
    pub width: f32,
    pub body: f32,
    pub place: Place,
    pub chrome: Chrome,
    pub escape: Escape,
    /// the last word of its accessible node: `window`
    pub node: &'a str,
    pub min_width: f32,
    pub min_body: f32,
    /// a scrim covers the main window while it is open
    pub modal: bool,
}

impl<'a> ChildSpec<'a> {
    /// A work surface: titled, centred, modal, dismissed by Escape unless a field is being typed in.
    pub fn work(key: &'a str, title: &'a str, width: f32, body: f32) -> Self {
        Self {
            key,
            title,
            width,
            body,
            place: Place::Centre,
            chrome: Chrome::Titled,
            escape: Escape::UnlessTyping,
            node: c::NODE,
            min_width: width,
            min_body: body,
            modal: true,
        }
    }
    /// One of the launcher's surfaces (`constants::child_window`).
    pub fn surface(key: &'a str, title: &'a str, s: c::Surface) -> Self {
        Self::work(key, title, s.1, s.2).least(s.3, s.4)
    }
    /// A tool beside the main window's corner, which keeps working (the popped-out build output).
    pub fn tool(key: &'a str, title: &'a str, width: f32, body: f32) -> Self {
        Self {
            place: Place::Corner,
            escape: Escape::Never,
            modal: false,
            ..Self::work(key, title, width, body)
        }
    }
    /// The least content it can be made: nothing in it is clipped at that size.
    pub fn least(self, width: f32, body: f32) -> Self {
        Self {
            min_width: width,
            min_body: body,
            ..self
        }
    }
    /// A message or question box: Escape closes it whatever has the keyboard.
    pub fn popup(self) -> Self {
        Self {
            escape: Escape::Always,
            ..self
        }
    }

    fn sized(&self, width: f32, body: f32) -> Vec2 {
        match self.chrome {
            Chrome::Titled => vec2(
                width + p::CHROME,
                p::TITLE_HEIGHT + p::GAP + body + p::CHROME,
            ),
            Chrome::Bare => vec2(width, body),
        }
    }
    /// The inner size when the content has all the room it wants.
    pub fn size(&self) -> Vec2 {
        self.sized(self.width, self.body)
    }
    pub fn min_size(&self) -> Vec2 {
        self.sized(self.min_width.min(self.width), self.min_body.min(self.body))
    }
}

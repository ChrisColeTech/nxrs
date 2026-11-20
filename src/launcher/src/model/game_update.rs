//! A game update the user installed (Game > Install Updates…): a package of its own, read in place.

use crate::constants::words_updates as w;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameUpdate {
    /// the package, absolute
    pub path: String,
    /// the game it updates, 16 hex digits
    pub title_id: String,
    /// its content version (262144); 0 when it has none (a bare NCA)
    pub version: i64,
    /// "1.1.1", from its control data; may be empty
    pub display: String,
}

impl GameUpdate {
    /// "1.1.1 (v262144)".
    pub fn label(&self) -> String {
        match (self.display.is_empty(), self.version != 0) {
            (true, true) => format!("v{}", self.version),
            (true, false) => w::UNKNOWN.into(),
            (false, true) => format!("{} (v{})", self.display, self.version),
            (false, false) => self.display.clone(),
        }
    }
}

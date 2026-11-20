//! The engine runs the main window keeps: a build, an update check, a DLC sync (one of each at most).

use crate::model::game::Game;
use crate::model::game_update::GameUpdate;
use crate::model::play::PlaySettings;
use crate::services::jobs::Job;

pub struct BuildRun {
    pub job: Job,
    /// the game as it was handed to the build (its layout resolved)
    pub game: Game,
    pub play: PlaySettings,
    /// the engine said the package also holds add-on content
    pub holds_dlc: bool,
}

pub struct ProbeRun {
    pub job: Job,
    pub game_id: String,
    pub found: Vec<GameUpdate>,
    pub errors: Vec<String>,
}

pub struct DlcRun {
    pub job: Job,
    pub game_id: String,
    pub errors: Vec<String>,
}

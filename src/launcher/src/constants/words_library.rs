//! What the library, sources and the main window say (`library.cpp`, `window.cpp`).

pub const INVALID_FILE: &str =
    "The library file is invalid or from an unsupported version. It was left untouched: ";
pub const NOT_WRITABLE: &str = "Library could not be loaded; refusing to overwrite it.";
pub const NO_DIRECTORY: &str = "Cannot create the library directory.";
pub const LOAD_FAILED: &str = "Library could not be loaded";
pub const SAVE_FAILED: &str = "Could not save library";
pub const NO_MAIN: &str = "This folder does not contain exefs/main or main.";
pub const NOT_CARD: &str = "{} is not a card image (no XCI header).";
pub const NOT_SOURCE: &str = "Select an indexed NUP, a standard application NSP, a card image (XCI), an NRO, or an extracted ExeFS folder.";
pub const NO_EXECUTABLE: &str = "The executable does not exist.";
pub const INCOMPLETE_BUILD: &str =
    "A native build needs its executable, matching code module, and _data folder together.";
pub const UNTITLED: &str = "Untitled game";
pub const DEFAULT_NAME: &str = "Game";
pub const CANNOT_ADD: &str = "Cannot add game";
pub const ALREADY_TITLE: &str = "Already in library";
pub const ALREADY: &str = "That source or native build is already in the library.";
pub const CANNOT_IMPORT: &str = "Cannot import build";
pub const BUSY_TITLE: &str = "Game is busy";
pub const BUSY_IMPORT: &str = "Wait for this game's build or run to finish before importing it.";
pub const REMOVE_TITLE: &str = "Remove from library";
pub const REMOVE_HEADING: &str = "Remove “{}” from the library?";
pub const REMOVE_TEXT: &str = "The game's package is never deleted.";
pub const REMOVE_VERB: &str = "Remove";
pub const DELETE_BUILD: &str = "Also delete the build ({})";
pub const DELETE_BUILD_TIP: &str = "Deletes the build folder {}. Saves, SD card files, cheats and \
    mods are moved first to {}; when the game is added and built again they go back into it. \
    Unticked, every file stays where it is.";
pub const DELETE_TITLE: &str = "Cannot delete the build";
pub const DELETE_BUSY: &str = "The game is running or building. Stop it, then remove it again.";
pub const KEPT: &str = "The saves of {} were kept in {}.";
pub const RESTORED: &str = "Kept saves put back into the build from {}";
// the build-folder migration (`--migrate-builds`, and at start)
pub const MOVED: &str = "Moved: {}: {} -> {}";
pub const MOVE_NOTHING: &str = "Updated: {}: nothing is built in {}; it builds into {}";
pub const MOVE_SKIPPED: &str = "Skipped: {}: {} stays (new place {}): ";
pub const MOVE_RUNNING: &str = "the game is running or building";
pub const MOVE_TAKEN: &str = "{} already exists";
pub const MOVE_IN_USE: &str = "its files are in use (the game may be running)";
pub const HOLDER_REMOVED: &str = "Removed the empty folder {}";
pub const HOLDER_LEFT: &str = "Left {}: other folders remain in it";
pub const MIGRATE_NONE: &str = "No build folder is in the old layout.";
pub const EMPTY: &str = "Your library is empty.\n\nAdd a NUP or NRO, import an existing build,\nor drop a game package into this window.";
pub const NO_MATCH: &str = "No games match these filters.\nPress Ctrl+F to adjust or clear them.";
pub const NO_SELECTION: &str = "No selection";
pub const NOT_LINKED: &str = "Not linked";
pub const GAMES: &str = "{} game{}";
pub const SHOWN_OF: &str = "{} / {} games";
pub const RESULT: &str = "{} of {} games shown";
pub const FILTERS_ACTIVE: &str = "{} active filters — Ctrl+F to edit or clear";
pub const ABOUT: &str = "Build and launch your own game dumps from one library.\nNo games, keys, or firmware are included.\n\nEngine: nxrs (MIT) — see legal/ in the release.\n\nNative controller input is supported. Vibration and some Switch 2 audio/video features remain incomplete.\n\nWebsite: https://nxrs.app";
/// About's own verbs (the footer's leading ghost buttons) and the pages they open in the browser.
pub const WEBSITE: &str = "Website";
pub const WEBSITE_URL: &str = "https://nxrs.app";
pub const GITHUB: &str = "GitHub";
pub const GITHUB_URL: &str = "https://github.com/ChrisColeTech/nxrs";
pub const ABOUT_LINKS: [(&str, &str); 2] = [(WEBSITE, WEBSITE_URL), (GITHUB, GITHUB_URL)];
pub const NO_ARTWORK: &str = "Add artwork";
pub const BAD_ARTWORK: &str = "Image unavailable";
pub const CHOOSE_ARTWORK: &str = "Choose artwork in Properties";
pub const ARTWORK_ERROR: &str = "Cannot load artwork: ";
pub const PICK_GAME: &str = "Add game";
pub const PICK_FOLDER: &str = "Add extracted game folder";
pub const GAME_FILTER: (&str, &[&str]) = ("Game packages", &["nup", "nsp", "xci", "nro"]);

/// The keys (`installKeys`).
pub const PICK_KEYS: &str = "Install Nintendo keys";
pub const KEYS_FILTER: (&str, &[&str]) = ("Nintendo key file (prod.keys)", &["keys"]);
pub const KEYS_INVALID_TITLE: &str = "Invalid key file";
pub const KEYS_INVALID: &str =
    "This file does not contain the NCA header keys required for NSP packages.";
pub const KEYS_FAILED: &str = "Cannot install keys";
pub const KEYS_NO_FOLDER: &str = "The launcher could not create its keys folder.";
pub const KEYS_DONE_TITLE: &str = "Keys installed";
pub const KEYS_DONE: &str = "prod.keys was installed in the launcher data folder. Key contents are never copied into builds or release packages.";

/// The statuses (`cardState`, `statusFor`).
pub const READY: &str = "Ready to play";
/// No native build to play, a web build that plays in the browser.
pub const READY_WEB: &str = "Ready to play in the browser";
pub const NEEDS_BUILD: &str = "Needs build";
pub const INCOMPLETE: &str = "Build incomplete";
pub const UPDATE_CHANGED: &str = "Update changed";
pub const UPDATE_CHANGED_LIST: &str = "Update changed — build again";
pub const OFFLINE: &str = "Source offline";
pub const BUILDING: &str = "Building";
pub const RUNNING: &str = "Running";
pub const PREVIEW: &str = "Preview";
pub const BUILD_NEEDED: &str = "Build needed";

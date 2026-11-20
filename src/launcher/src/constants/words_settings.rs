//! What Properties and Settings say (`window.cpp` `editGame`, `showSettings`).

pub const PROPERTIES_TITLE: &str = " — Properties";
pub const ADD_TITLE: &str = "Add game";
pub const PROPERTIES_TABS: [&str; 6] = ["General", "Build", "Runtime", "Artwork", "DLC", "Updates"];
pub const SETTINGS_TITLE: &str = "nxrs — Settings";
pub const SETTINGS_TABS: [&str; 5] = [
    "Engine",
    "Build defaults",
    "Controller",
    "Appearance",
    "Play",
];
pub const APPLY: &str = "Apply";
pub const CANCEL: &str = "Cancel";
pub const CLOSE: &str = "Close";
pub const YES: &str = "Yes";
pub const NO: &str = "No";
pub const OK: &str = "OK";

pub const MISSING_TITLE: &str = "Missing details";
pub const MISSING: &str = "A title and build folder are required.";
pub const ARTWORK_TITLE: &str = "Cannot load artwork";
pub const PATHS_NOTE: &str =
    "NUP/NSP game data stays in the original package. Changing the source requires a rebuild.";
pub const RUNTIME_NOTE: &str = "Other runtime options remain in the game's INI; the background settings are in Settings > Play. The screen mode and the controller layout are saved right away (their defaults are in Settings > Play and Settings > Controller).";
pub const ARTWORK_NOTE: &str = "The background follows your selected game and is dimmed behind the gallery. Leave it empty to use the cover. Clear both paths to restore generated artwork.";
pub const ENGINE_NXRS: &str = "nxrs";
pub const ENGINE_TURBO: &str =
    "Turbo — no longer supported: Build makes an nxrs build (in the [nxrs] folder)";
pub const VULKAN: &str = "Vulkan";
pub const USE_DEFAULT: &str = "Use default";
pub const NOT_BUILT: &str = "Not built";
pub const CHOOSE_FOLDER: &str = "Choose extracted folder…";
pub const OPEN_INI: &str = "Open game settings file";

pub const LAYOUT_SWITCH: &str = "Switch — by position (A right, B bottom)";
pub const LAYOUT_XBOX: &str = "Xbox — by label (A bottom, B right)";
pub const LAYOUT_DEFAULT_SWITCH: &str = "Use default — Switch";
pub const LAYOUT_DEFAULT_XBOX: &str = "Use default — Xbox";
pub const SCREEN_TV: &str = "TV mode — sharper";
pub const SCREEN_HANDHELD: &str = "Handheld — lighter";
pub const SCREEN_DEFAULT_TV: &str = "Use default — TV mode";
pub const SCREEN_DEFAULT_HANDHELD: &str = "Use default — Handheld";
pub const SCREEN_TIP: &str = "TV mode renders at the game's TV resolution (Switch 1920x1080, Switch 2 up to 2560x1440 and more): sharper. Handheld renders at its handheld resolution (1280x720 / 1920x1080): lighter on the GPU. The default is in Settings > Play.";

pub const ENGINE_ROW: &str = "Engine";
pub const CHECKS_ROW: &str = "Checks";
pub const NXRS_PLACEHOLDER: &str = "The engine bundled with this launcher";
pub const NXRS_PICK: &str = "Choose the engine (a development build)";
pub const CHECK_AGAIN: &str = "Check again";
pub const OUTPUT_PLACEHOLDER: &str = "Empty: create builds beside the source package";
pub const CONTROLLER_NOTE: &str = "Applies to Xbox, PlayStation and other controllers. Switch keeps the Switch pad's positions, so on a PlayStation pad Circle is A and Cross is B. Xbox follows the printed Xbox letters: Cross is A, Square is X.\n\nSaved into every built game that uses the default (a running game switches live). Override it per game in Properties > Runtime; single buttons can be remapped with button_map in the game's INI.";
pub const BACKDROP: &str = "Show selected game artwork behind the gallery";
pub const LIBRARY_FILE: &str = "Library file:\n";
pub const PAUSE: &str = "Pause the game when its window is in the background";
pub const MUTE: &str = "Mute the game when its window is in the background";
pub const ASK_USER: &str = "Ask who is playing (when several profiles exist)";
pub const PLAY_NOTE: &str = "The screen mode is the default for every game without its own (Properties > Runtime); the rest apply to every game. Off: a game keeps running, and its sound playing, while you use another window, as a PC game does. Built games get a change in their settings file right away, the others when they are built.";
pub const BUSY_TITLE: &str = "Build in progress";
pub const BUSY: &str =
    "Engine/build defaults cannot change during a build. Appearance can still be changed.";
pub const NXRS_MISSING_TITLE: &str = "Engine not found";
pub const NXRS_MISSING: &str = "Select nxrs-build{} or the folder that holds it.";
pub const LAYOUT_FAILED: &str = "The controller layout could not be written to {} game(s): {}\nIt is written again when you play.";
pub const PLAY_FAILED: &str = "The play settings could not be written to {} game(s): {}\nThey are written again when you play.";

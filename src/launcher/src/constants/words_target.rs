//! What Play and Build say about their two versions, Native and Web (owner, 2026-11-27): Play runs the
//! one version there is and asks only when both are there; Build always asks.

/// The dialog's window title.
pub const PLAY: &str = "Play";
pub const BUILD: &str = "Build";
/// Its heading (the task, the game) and the sentence under it.
pub const PLAY_HEADING: &str = "Play {}";
pub const BUILD_HEADING: &str = "Build {}";
pub const PLAY_INTRO: &str = "This game has two versions. Choose where to run it.";
pub const BUILD_INTRO: &str = "Choose which version to build.";
/// The heading's info mark.
pub const PLAY_TIP: &str = "Native is the Windows program the launcher built; Web is the version \
                            that runs in a Chromium browser, served by the launcher from this PC.";
pub const BUILD_TIP: &str = "Each version is built into its own folder; building one leaves the \
                             other as it is.";

/// Each option: its name, what it is, its button.
pub const NATIVE: &str = "Native";
pub const WEB: &str = "Web";
pub const PLAY_NATIVE_WHAT: &str = "Runs as a Windows program on this PC.";
/// The browser.
pub const PLAY_WEB_WHAT: &str = "Runs in {}, served from this PC by the launcher.";
pub const BUILD_NATIVE_WHAT: &str = "Builds the Windows program the launcher runs.";
pub const BUILD_WEB_WHAT: &str = "Builds a version that runs in Chrome.";
pub const PLAY_NATIVE: &str = "Play native";
/// The browser.
pub const PLAY_WEB: &str = "Play in {}";
pub const BUILD_NATIVE: &str = "Build native";
pub const BUILD_WEB: &str = "Build for web";

/// Each option's state line.
/// When it was built ("26 Nov 2026, 9:14 PM").
pub const BUILT: &str = "Built {}";
pub const BUILT_AGAIN: &str = "Built {}. Building again replaces it.";
pub const NOT_BUILT: &str = "Not built yet";

/// Why an option cannot run now, in its state line.
pub const NO_NATIVE_BUILD: &str = "No native build yet: build it first.";
pub const NO_WEB_BUILD: &str = "No web build yet: build it for the web first.";
pub const NO_BROWSER: &str = "No Chromium browser found: install Chrome.";
pub const RUNNING: &str = "The game is running.";
pub const BUILDING: &str = "The game is building.";
pub const UP_TO_DATE: &str = "The native build is up to date.";
pub const OTHER_BUILD: &str = "Another build is running.";
pub const NO_SOURCE: &str = "The source package is missing.";
pub const PREVIEW: &str = "Not in the preview.";

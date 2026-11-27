//! What Play and Build ask when a game has two versions, Native and Web (owner, 2026-11-27: one Play,
//! one Build; Play with one version runs it). The window titles are `words_contexts`' Play and Build.

/// The question (the heading), the game's title in it.
pub const PLAY_HEADING: &str = "Play {} natively or on the web?";
pub const BUILD_HEADING: &str = "Build {} natively or for the web?";
/// The text under it; Play's names the browser.
pub const PLAY_TEXT: &str = "Native runs the Windows program the launcher built. Web runs the web \
                             build in {}, served from this PC by the launcher.";
pub const BUILD_TEXT: &str = "Native builds the Windows program into the game's build folder. Web \
                              builds the version that runs in Chrome into its web folder. Building \
                              one leaves the other as it is.";
/// The two equal options, side by side in this order.
pub const NATIVE: &str = "Native";
pub const WEB: &str = "Web";

/// Why a version cannot play or build now (its disabled button's tooltip).
pub const NO_NATIVE_BUILD: &str = "No native build yet: build it first.";
pub const NO_WEB_BUILD: &str = "No web build yet: build it for the web first.";
pub const NO_BROWSER: &str = "No Chromium browser found: install Chrome.";
pub const RUNNING: &str = "The game is running.";
pub const BUILDING: &str = "The game is building.";
pub const UP_TO_DATE: &str = "The native build is up to date.";
pub const OTHER_BUILD: &str = "Another build is running.";
pub const NO_SOURCE: &str = "The source package is missing.";
pub const PREVIEW: &str = "Not in the preview.";

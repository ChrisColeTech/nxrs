//! What Build and Play say when their chooser's answer is Web.

pub const OUTPUT: &str = "Web build output";
pub const NO_FOLDER: &str = "Cannot create the web build folder.";
pub const DONE: &str = "Web build complete. Play > Web serves it from this computer.";
/// The status bar after a web build (the native build's "ready to play" would mislead).
pub const JOB_DONE: &str = "Web build complete — Play > Web";
pub const MISSING_FILES: &str =
    "nxrs-build finished, but the web build is incomplete (no build.json or game.wasm).";
pub const NOT_SERVED_TITLE: &str = "Cannot play in the browser";
pub const NO_SERVER: &str = "The local web server could not start: ";
pub const BAD_BUILD: &str = "This web build cannot be served: ";
pub const NO_BROWSER: &str = "Could not open the browser: ";
/// The browser, the address.
pub const OPENED: &str = "Opened in {}: {}";
/// The port asked for, the one taken: the browser's data cache and saves are kept per address.
pub const OTHER_PORT: &str = "Port {} was taken, so the web server uses {}: the browser keeps \
                              game data and saves per address, so they start empty there.";
/// The folder, the address.
pub const SERVING: &str = "Serving {} at {} (Ctrl+C to stop)";
pub const SERVE_FAILED: &str = "Cannot serve the web build: ";

pub const NOT_JSON: &str = "build.json is not valid JSON";
pub const NOT_WEB: &str = "build.json does not describe an nxrs web build";
pub const NEWER: &str = "build.json is from a newer nxrs-build (format {}); update the launcher";
pub const NO_DATA: &str = "build.json names no data folder";
pub const NO_DATA_DIR: &str = "the data folder is missing: ";
pub const NO_BUILD_JSON: &str = "no build.json (build it for web again)";
pub const NO_PAGE: &str = "the game's page is not there: ";
pub const DEFAULT_BROWSER: &str = "the default browser";

/// The shader translation route's answers (the page puts them in its red line).
pub const NO_TRANSLATOR: &str = "this server has no nxrs-build to translate shaders with";
pub const BAD_SHADER_REQUEST: &str = "not a shader request (u32 ctl length, ctl, code)";
pub const XLATE_LATE: &str = "nxrs-build shader-xlate did not finish in time";
pub const XLATE_NOT_STARTED: &str = "nxrs-build shader-xlate did not start: ";
/// The key nxrs-build printed, the key asked for.
pub const XLATE_OTHER_KEY: &str = "nxrs-build translated key {} for {}";
/// `--serve-web`: the nxrs-build the page's run-time shaders are translated with.
pub const XLATE_WITH: &str = "Run-time shaders are translated with {}";
/// The server's own line per request: slug, key, what happened, seconds.
pub const XLATE_LINE: &str = "shader-xlate {} {}: {} in {} s";

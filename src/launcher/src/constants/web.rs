//! The web build (docs/35 W5): its folder, its description file, the local server and the browsers
//! that play it.

/// A game's web build goes into `<base>\nxrs-web\<name>`, beside its `<base>\nxrs\<name>`.
pub const WEB_BUILDS_FOLDER: &str = "nxrs-web";
/// What nxrs-build writes into a finished web build (src/cli/src/webhost.rs).
pub const BUILD_JSON: &str = "build.json";
/// The newest `build.json` layout this launcher reads.
pub const BUILD_JSON_FORMAT: u64 = 1;
pub const BUILD_JSON_KIND: &str = "nxrs-web";
/// The module a finished web build has beside `build.json`.
pub const MODULE: &str = "game.wasm";
pub const DEFAULT_ENTRY: &str = "index.html";
/// The engine's build log of a web build, in its folder.
pub const BUILD_LOG_SUFFIX: &str = ".build.log";
/// `--target web` (launcher contract v2).
pub const TARGET_ARGS: [&str; 2] = ["--target", "web"];
/// `--shaders-from <native data folder>`: the shaders the native build's runs translated (a title
/// that makes its shaders at run time has none the web build can find or translate in a browser).
pub const SHADERS_FROM_ARG: &str = "--shaders-from";

/// The server's port: fixed, so the page's origin (and with it the browser's data cache and saves,
/// both kept per origin) stays the same from one Play to the next. `NXRS_WEB_PORT` overrides it.
pub const DEFAULT_PORT: u16 = 8780;
pub const ENV_PORT: &str = "NXRS_WEB_PORT";
/// Ports tried after a taken one.
pub const PORT_TRIES: u16 = 10;
pub const HOST: &str = "127.0.0.1";
/// A request head larger than this is refused.
pub const MAX_HEAD: usize = 16 * 1024;
/// An idle keep-alive connection is closed after this long.
pub const IDLE_SECONDS: u64 = 120;
/// Bytes copied from a file per write.
pub const CHUNK: usize = 1 << 20;
/// Sent with every answer: shared memory and threads need a cross-origin isolated page.
pub const ISOLATION_HEADERS: [(&str, &str); 3] = [
    ("Cross-Origin-Opener-Policy", "same-origin"),
    ("Cross-Origin-Embedder-Policy", "require-corp"),
    ("Cross-Origin-Resource-Policy", "same-origin"),
];
/// URL path segment that maps to the build's data folder.
pub const DATA_SEGMENT: &str = "data";

/// Windows: the `http` association's ProgIds of Chromium browsers (the default browser plays the
/// build when it is one of these), with a name for the status line.
pub const CHROMIUM_PROG_IDS: [(&str, &str); 7] = [
    ("ChromeHTML", "Chrome"),
    ("ChromeBHTML", "Chrome Beta"),
    ("MSEdgeHTM", "Edge"),
    ("BraveHTML", "Brave"),
    ("ChromiumHTM", "Chromium"),
    ("VivaldiHTM", "Vivaldi"),
    ("OperaStable", "Opera"),
];
pub const USER_CHOICE_KEY: &str =
    r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\http\UserChoice";
pub const CHROME_APP_PATH_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe";
/// Chrome's own install folders, under Program Files, Program Files (x86) and the user's local data.
pub const CHROME_EXE: &str = r"Google\Chrome\Application\chrome.exe";
pub const CHROME_BASE_VARS: [&str; 3] = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"];
/// Linux: Chromium browsers looked for on PATH, in this order.
pub const LINUX_CHROMES: [&str; 4] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
];
pub const CHROME: &str = "Chrome";
/// `NXRS_BROWSER`: a program Play in browser starts with the address instead of the browser found
/// (development and verification, like `NXRS_BUILD`).
pub const ENV_BROWSER: &str = "NXRS_BROWSER";

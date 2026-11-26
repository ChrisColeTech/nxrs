//! nxrs-build's launcher contract and the names the build folder uses.

pub const ENGINE_NXRS: &str = "nxrs";
/// A build goes into `<source folder or Build location>\nxrs\<name>` (owner, 2026-11-26).
pub const BUILDS_FOLDER: &str = "nxrs";
/// The layout before: `<source folder>\SwitchRecomp Builds\<name> [nxrs]` (moved by the migration).
pub const LEGACY_BUILDS: &str = "SwitchRecomp Builds";
pub const LEGACY_SUFFIX: &str = " [nxrs]";
/// `nxrs-build contract` answers `nxrs-contract <n>`; this launcher needs 2 (subcommands + `@` lines).
pub const CONTRACT_NEEDED: i64 = 2;
pub const NXRS_BUILD_NAME: &str = "nxrs-build";
/// The release folder beside the launcher that holds nxrs-build and `engine.json`.
pub const BUNDLED_FOLDER: &str = "nxrs-engine";
pub const MANIFEST: &str = "engine.json";
/// `NXRS_BUILD`: a development nxrs-build (a file or its folder).
pub const ENV_NXRS_BUILD: &str = "NXRS_BUILD";
/// `NXRS_DATA_DIR`: the launcher's data folder instead of `%LOCALAPPDATA%\nxrs` (development, tests).
pub const ENV_DATA_DIR: &str = "NXRS_DATA_DIR";
pub const ENV_KEYS: &str = "NXRS_PROD_KEYS";
pub const DATA_FOLDER: &str = "nxrs";
pub const LIBRARY_FILE: &str = "library.json";
pub const QT_BACKUP: &str = "library.qt-backup";
pub const KEYS_FILE: &str = "keys/prod.keys";
pub const LEGACY_FOLDERS: [&str; 2] = [
    "RecompLauncher/RecompLauncher",
    "RecompLauncher/SwitchRecompLauncher",
];
/// The player's files of a removed game wait in `<data folder>\saves\<title id>` (kept_saves).
pub const SAVES_FOLDER: &str = "saves";
/// The player's folders in a data folder (saves, SD card, cheats), and the build's mods folder.
pub const PLAYER_FOLDERS: [&str; 3] = ["save", "sdmc", "cheats"];
pub const MODS_FOLDER: &str = "mods";
pub const NPDM: &str = "exefs/main.npdm";
pub const NOT_A_BUILD: &str = "Not deleted: this folder is not a build of this game, or it holds the \
                               game's package: ";
/// Before the migration's first write, the library file is copied here (once).
pub const PRE_LAYOUT_BACKUP: &str = "library.pre-layout.json";

#[cfg(windows)]
pub const EXECUTABLE_SUFFIX: &str = ".exe";
#[cfg(not(windows))]
pub const EXECUTABLE_SUFFIX: &str = "";
#[cfg(windows)]
pub const MODULE_SUFFIX: &str = "_code.dll";
#[cfg(not(windows))]
pub const MODULE_SUFFIX: &str = "_code.so";

/// The `@progress` stages and what the launcher calls them.
pub const STAGES: [(&str, &str); 7] = [
    ("open", "Opening the game"),
    ("analyze", "Analyzing ARM64 code"),
    ("translate", "Generating native C"),
    ("compile", "Compiling native code"),
    ("link", "Linking native module"),
    ("shaders", "Translating shaders"),
    ("data", "Preparing game data"),
];
pub const STARTING: &str = "Starting recompiler";
pub const STOPPING: &str = "Stopping build processes";
pub const PACKAGE_DLC_LINE: &str = "DLC: this package also holds add-on content";
pub const DLC_ERROR: &str = "DLC error: ";
pub const UPDATE_ERROR: &str = "UPDATE error: ";
/// Covers found in a native build's data folder, in this order.
pub const BUILD_ICONS: [&str; 3] = ["icon.jpg", "icon.png", "icon_AmericanEnglish.dat"];
/// The engine's own folders, in a game's data folder.
pub const NACP: &str = "control.nacp";
pub const NACP_VERSION_AT: u64 = 0x3060;
pub const DLC_MANIFEST: &str = "dlc.manifest";
/// What the build's extracted RomFS was made from (the old marker's text): the package, the update.
pub const ROMFS_SOURCE: &str = "romfs.source";
/// The in-place markers of builds made before the build carried its data (read from the package).
pub const ROMFS_NUP: &str = "romfs.nup";
pub const ROMFS_NSP: &str = "romfs.nsp";
pub const ROMFS_DIR: &str = "romfs.dir";

//! What add-on content says (`window.cpp`, `dlcpanel.cpp`, `library.cpp`).

pub const SELECT_FIRST: &str = "Select a game first.";
pub const CANNOT_INSTALL: &str = "Cannot install DLC";
pub const CANNOT_REMOVE: &str = "Cannot remove DLC";
pub const BUSY_TITLE: &str = "Game is busy";
pub const BUSY: &str = "Wait for this game's build, run or DLC installation to finish first.";
pub const ALREADY_TITLE: &str = "DLC already installed";
pub const ALREADY: &str = "Those packages are already in this game's DLC list.";
pub const ADDED_TITLE: &str = "DLC added";
pub const ADDED: &str = "{} package{} added. The DLC is installed when {} is built.";
pub const NOT_LISTED_TITLE: &str = "DLC not installed";
pub const NOT_LISTED: &str = "That package is not in this game's DLC list.";
pub const REMOVED_TITLE: &str = "DLC removed";
pub const REMOVED: &str = "{} package{} taken off the list. The original files were not changed.";
pub const SYNC_BUSY: &str = "Another DLC installation is already running.";
pub const BUILD_FIRST: &str = "Build the game first: DLC is installed into its data folder.";
pub const NEEDS_KEYS: &str = "Installing DLC needs prod.keys. Use Keys > Install keys first.";
pub const JOB_UPDATING: &str = "Updating DLC…";
pub const JOB_INSTALLING: &str = "Installing DLC…";
pub const BUSY_REMOVING: &str = "Removing…";
pub const BUSY_INSTALLING: &str = "Installing… a large package can take a while.";
pub const BUSY_RUNNING: &str = "A DLC installation is running…";
pub const FAILED_TITLE: &str = "DLC could not be installed";
pub const JOB_FAILED: &str = "DLC was not installed — see the message";
pub const JOB_REMOVED: &str = "DLC removed";
pub const JOB_INSTALLED: &str = "DLC installed — {} title{}";
pub const INSTALLED_TITLE: &str = "DLC installed";
pub const INSTALLED_SHORT: &str = "Installed: {}. It is in effect the next time you play.";
pub const INSTALLED_LONG: &str = "{} now has this add-on content:\n\n{}\n\nIt is in effect the next time you play. Game > Install DLC… lists it and removes it.";
pub const REMOVED_NONE: &str =
    "Removed. No DLC is installed now; the original files were not changed.";
pub const REMOVED_SOME: &str = "Removed. Still installed: {}. The original files were not changed.";
pub const QUIET_NONE: &str = "No DLC is installed now.";
pub const QUIET_SOME: &str = "Installed: {}";
pub const STOPPED: &str = "The engine stopped (exit {}). See the console for details.";

pub const WINDOW_TITLE: &str = "DLC — ";
pub const ADD: &str = "Add DLC…";
pub const ADD_TIP: &str = "Install one or more DLC packages (NSP, NSZ, XCI, XCZ, NCA)";
pub const REMOVE: &str = "Remove";
pub const REMOVE_TIP: &str =
    "Uninstall the selected DLC (Delete). The original files are not changed.";
pub const REMOVE_MANY: &str = "Remove {} packages";
pub const OPEN_FOLDER: &str = "Open containing folder";
pub const LIST_NAME: &str = "Installed DLC";
pub const PICKER: &str = "Add DLC";
pub const FILTER: (&str, &[&str]) = ("Add-on content", &["nsp", "nsz", "xci", "xcz", "nca"]);
pub const HINT_LOCKED: &str = "DLC cannot be changed while the game is running or building.";
pub const HINT_EMPTY: &str = "No DLC is installed for this game. Click Add DLC… or drop files here: NSP, NSZ, XCI, XCZ or NCA, several at once; a package that also holds the game or its update works too.";
pub const HINT_BUILT: &str = "Add DLC… installs more packages, several at once. Select one or more entries and click Remove to uninstall them. The original files are never changed.";
pub const HINT_UNBUILT: &str = "These packages are installed when the game is built. Select entries and click Remove to take them off the list.";
pub const PANEL_EMPTY: &str = "No DLC is installed for this game. Add it with Game > Install DLC… (NSP, NSZ, XCI, XCZ or NCA; a package that also holds the game or its update works too).";
pub const PANEL_NOTE: &str = "Right-click an entry and choose Remove (or press Delete) to uninstall it; it takes effect when you click Apply. The original files are never changed. Add more with Game > Install DLC…";

/// Package checks (`validDlcPackage`) and the list's descriptions (`dlcDescription`).
pub const NO_FILE: &str = "The file does not exist: ";
pub const NOT_PACKAGE: &str =
    "{} is not an add-on content package. Select an NSP, NSZ, XCI, XCZ or NCA file.";
pub const BAD_NSP: &str = "{} is not a valid NSP/NSZ package (no PFS0 header).";
pub const BAD_XCI: &str = "{} is not a valid XCI/XCZ card image.";
pub const SMALL_NCA: &str = "{} is too small to be an NCA.";
pub const FILE_NOT_FOUND: &str = " — file not found";
pub const NOT_INSTALLED_YET: &str = " — not installed yet";
pub const WHEN_BUILT: &str = " — installs when the game is built";
pub const NO_DATA: &str = " (no data)";

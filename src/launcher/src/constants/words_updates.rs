//! What game updates say (`window.cpp`, `updatepanel.cpp`, `library.cpp`).

pub const CANNOT_INSTALL: &str = "Cannot install the update";
pub const CANNOT_REMOVE: &str = "Cannot remove the update";
pub const CANNOT_CHOOSE: &str = "Cannot choose the update";
pub const BUSY: &str = "Wait for this game's build, run or update check to finish first.";
pub const BUSY_RUN: &str = "Wait for this game's build or run to finish first.";
pub const ALREADY_TITLE: &str = "Update already installed";
pub const ALREADY: &str = "Those packages are already on this game's update list.";
pub const NEEDS_KEYS: &str = "Installing an update needs prod.keys. Use Keys > Install keys first.";
pub const NO_SOURCE: &str =
    "The game's own package is missing; it is needed to check that the update belongs to it.";
pub const JOB_CHECKING: &str = "Checking updates…";
pub const BUSY_CHECKING: &str = "Checking the update against the game…";
pub const BUSY_OTHERS: &str = "\nChecking the others…";
pub const BUSY_RUNNING: &str = "Updates are being checked…";
pub const ADDED_TITLE: &str = "Update added";
pub const FAILED_TITLE: &str = "Update could not be added";
pub const ADDED: &str = "Added: {}. {}";
pub const NOT_LISTED_TITLE: &str = "Update not installed";
pub const NOT_LISTED: &str = "That package is not on this game's update list.";
pub const REMOVED_TITLE: &str = "Update removed";
pub const REMOVED: &str = "Removed {} update{}; the original file{} not changed. {}";
pub const CHOSEN_TITLE: &str = "Version chosen";
pub const OUTCOME_PENDING: &str = "Build the game again to get {} (Build now, or Rebuild on its card); until then it cannot be played.";
pub const OUTCOME_BUILT: &str = "The current build already is {}.";
pub const OUTCOME_NEXT: &str = "The next build will be {}.";
pub const PENDING: &str =
    "The current build is {}. Build the game again to get {}: an update replaces the game's code.";

pub const WINDOW_TITLE: &str = "Updates — ";
pub const VERSION: &str = "Version to build";
pub const VERSION_TIP: &str =
    "Which update the game is built with. One update applies at a time; the newest is the default.";
pub const NEWEST: &str = "Newest installed update";
pub const WITHOUT: &str = "The game without updates";
pub const ADD: &str = "Add update…";
pub const ADD_TIP: &str = "Install one or more update packages (NSP, XCI, NCA)";
pub const REMOVE: &str = "Remove";
pub const REMOVE_TIP: &str =
    "Take the selected updates off the list (Delete). The original files are not changed.";
pub const REMOVE_MANY: &str = "Remove {} updates";
pub const BUILD_WITH: &str = "Build with this update";
pub const BUILD_NOW: &str = "Build now";
pub const BUILD_NOW_TIP: &str = "Build the game again with the version chosen above";
pub const LIST_NAME: &str = "Installed updates";
pub const PICKER: &str = "Add update";
pub const FILTER: (&str, &[&str]) = ("Game updates", &["nsp", "xci", "nca"]);
pub const HINT_LOCKED: &str = "Updates cannot be changed while the game is running or building.";
/// The list's state under it when nothing is installed (how to add is in the heading's info mark).
pub const LIST_EMPTY: &str = "No update is installed for this game.";
pub const HINT_EMPTY: &str = "No update is installed for this game. Click Add update… or drop files here: NSP, XCI or NCA, several at once. The game itself has to be an NSP package; a package that holds another game's update is refused.";
pub const HINT_SOME: &str = "Add update… installs more packages, several at once. Select one or more entries and click Remove to take them off. One update applies at a time: the newest, unless Version to build says otherwise. The original files are never changed; a build copies the update's data into the game, which then plays without the package.";
pub const PANEL_EMPTY: &str = "No update is installed for this game. Add one with Game > Install Updates… (NSP, XCI or NCA; the game itself has to be an NSP package).";
pub const PANEL_NOTE: &str = "Right-click an entry and choose Remove (or press Delete) to take it off; it takes effect when you click Apply and needs the game built again. Add updates and choose the version in Game > Install Updates…";

/// Labels (`updateTokenLabel`, `updateDescription`) and package checks (`validUpdatePackage`).
pub const WITHOUT_LABEL: &str = "the game without updates";
pub const AS_IS: &str = "the game as its package is";
pub const UNKNOWN: &str = "version unknown";
pub const APPLIED: &str = " — applied";
pub const SELECTED: &str = " — selected, build the game to apply it";
pub const IN_BUILD: &str = " — in the current build, not selected";
pub const COMPRESSED: &str = "{} is a compressed package. An update is read in place by the build, which a compressed file does not allow: decompress it to an NSP or XCI first (nsz -D).";
pub const NOT_UPDATE: &str = "{} is not an update package. Select an NSP, XCI or NCA file.";
pub const BAD_NSP: &str = "{} is not a valid NSP package (no PFS0 header).";
pub const BAD_XCI: &str = "{} is not a valid XCI card image.";

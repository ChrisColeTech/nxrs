//! What the artwork copies and the import of web builds say.

/// The image's path.
pub const ART_MISSING: &str = "the image is not there: {}";
pub const ART_UNREADABLE: &str = "the image could not be copied: {}";
/// The game, cover or background, why.
pub const ART_LEFT: &str = "Artwork of {} ({}) left where it is: {}";
/// The copies made at start: how many images.
pub const ART_ADOPTED: &str = "Artwork: {} images copied into {}";
/// The backup's path.
pub const BACKUP_FAILED: &str =
    "Artwork was not moved into the launcher: the library could not be backed up to {}";

/// Import build's picker: native executables and web builds.
pub const PICK_BUILD: &str = "Import build";
pub const BUILD_FILTER: (&str, &[&str]) = ("Builds", &["exe", "html", "htm", "json"]);
pub const NOT_A_BUILD: &str = "Choose a native build's executable, or a web build: its folder, its \
                               build.json or its page.";
pub const NO_PAGE: &str =
    "This folder has no build.json and no index.html (looked {} folders deep).";
pub const SEVERAL_PAGES: &str =
    "This folder holds more than one index.html; choose the game's page itself: ";

//! What Play, Stop, folders and saves say (`window.cpp`, `library.cpp`).

pub const OFFLINE_TITLE: &str = "Source unavailable";
pub const OFFLINE: &str = "Reconnect the drive containing the source package, or update the source in Properties and rebuild.";
pub const NO_SETTINGS: &str = "Cannot save game settings";
pub const NOT_LAUNCHED: &str = "Could not launch";
pub const LAUNCHED: &str = "Game launched";
pub const CLOSE_ASKED: &str = "Close requested — waiting for game";
pub const NO_CLOSE_TITLE: &str = "Could not close game";
pub const NO_CLOSE: &str = "Close the game from its own window.";
pub const NO_FILE_TITLE: &str = "File not available";
pub const NO_FILE: &str = "Run this game once to create its log and settings.";
pub const NO_FOLDER_TITLE: &str = "Folder not available";
pub const NO_SAVES: &str = "This game has not created a save folder yet.";
pub const NO_BUILD: &str = "Build this game first to create its output folder.";

pub const COPY_TITLE: &str = "Copy saves";
pub const COPY_OFFER: &str = "The Turbo build of “{}” has saves. Copy them into this build before playing?\n\nThis build's save folder is backed up first (save-backup-<time> beside it). Asked once; Game > Copy saves does it later.";
pub const COPY_ASK: &str = "Copy the saves of the Turbo build of “{}” into this build?\n\nThis build's save folder is backed up first; the Turbo build's saves are only read.";
pub const COPY_FAILED: &str = "Cannot copy saves";
pub const COPY_SELECT: &str = "Select a game first.";
pub const COPY_BUSY: &str = "Close the game (or wait for its build) first.";
pub const COPIED: &str = "Saves copied";
pub const NO_BACKUP: &str = "\nThis build had no saves; nothing to back up.";
pub const BACKUP: &str = "\nBackup of this build's saves: ";
pub const NOT_NXRS: &str =
    "Build the game with the nxrs engine first: its saves go into the nxrs build.";
pub const NO_DATA: &str =
    "Build the game with this engine first: its data folder does not exist yet.";
pub const NOTHING: &str = "The Turbo build has no saves to copy ({}).";
pub const BACKUP_EXISTS: &str = "A backup with this time already exists: ";
pub const CANNOT_CREATE: &str = "Cannot create ";
pub const CANNOT_REPLACE: &str = "Cannot replace ";
pub const CANNOT_COPY: &str = "Cannot copy ";
pub const CANNOT_REMOVE: &str = "Cannot remove ";

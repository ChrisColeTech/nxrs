//! What a build says (`buildmanager.cpp`, `window.cpp`).

pub const BUSY: &str = "Another build is already running.";
pub const NEEDS_KEYS: &str = " builds need prod.keys. Use Keys > Install keys first.";
pub const UPDATE_MISSING: &str =
    "The update {} is missing. Put the file back, or remove it in Game > Install Updates.";
pub const UPDATE_KEYS: &str = "Applying an update needs prod.keys. Use Keys > Install keys first.";
pub const NO_FOLDER: &str = "Cannot create the build output folder.";
pub const NOT_STARTED: &str = "nxrs-build did not start: ";
pub const CANCELLED: &str =
    "Build cancelled. Saves were not deleted. Rebuild before playing again.";
pub const FAILED: &str = "Build failed (exit {}). See the build log for details.";
pub const MISSING_FILES: &str =
    "The recompiler exited successfully but the expected build files are missing.";
pub const NO_SETTINGS: &str = "Build completed, but settings could not be saved: ";
pub const DONE: &str = "Build complete. Your game is ready to play.";
pub const CANNOT_START: &str = "Cannot start build";
pub const LOG_HEADER: &str = "nxrs build · ";
pub const OUTPUT: &str = "Build output";
pub const OUTPUT_COMPLETE: &str = "Build output — complete";
pub const OUTPUT_STOPPED: &str = "Build output — stopped";
pub const JOB_COMPLETE: &str = "Build complete — ready to play";
pub const JOB_STOPPED: &str = "Build stopped — see output";
pub const PLACEHOLDER: &str = "Build output appears here. Select a game and choose Build to begin.";
pub const PACKAGE_DLC: &str = "DLC: {} is now on this game's DLC list";
pub const CLOSE_TITLE: &str = "Build in progress";
pub const CLOSE_TEXT: &str = "Cancel the build and close the launcher?";
pub const CLOSE_VERB: &str = "Stop build and close";

/// nxrs-build's problems (`Toolchain::nxrsProblem`).
pub const NOT_FOUND: &str = "nxrs-build was not found. It ships in the nxrs-engine folder beside nxrs; for a development engine choose it in File > Settings > Engine (or set NXRS_BUILD).";
pub const OLD_CONTRACT: &str =
    "{} answers launcher contract {}; this launcher needs {}. Use a newer nxrs-build.";
pub const PROBLEM_PREFIX: &str = "nxrs engine: ";
pub const DOCTOR_NONE: &str =
    "nxrs-build doctor printed no checks (an engine older than the doctor command?)";
pub const DOCTOR_MISSING: &str = "nxrs-build not found";
pub const DOCTOR_LATE: &str = "nxrs-build doctor did not finish in time";
pub const DOCTOR_CHECKING: &str = "Checking…";
pub const DOCTOR_NO_ENGINE: &str = "Engine not found: no checks.";
pub const DOCTOR_PASSED: &str = "All checks passed.";
pub const DOCTOR_FAILED: &str = "{} check(s) failed.";
pub const BADGE_READY: &str = "Engine ready";
pub const BADGE_PROBLEM: &str = "Setup needed";
pub const BADGE_CHECKING: &str = "Checking…";
pub const POPUP_READY: &str = "Build tools";
pub const POPUP_PROBLEM: &str = "Build setup problem";
pub const OPEN_SETTINGS: &str = "Open Settings";
pub const MISSING: &str = "Missing";

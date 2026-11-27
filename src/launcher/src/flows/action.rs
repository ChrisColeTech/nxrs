//! Every user command: menus, context menus, shortcuts, card buttons and command-line flags produce
//! these, and `dispatch` handles them (the Qt `QAction`s).

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    AddGame,
    AddFolder,
    ImportBuildPick,
    Settings(usize),
    Exit,
    /// Play: asks Native or Web (the target chooser)
    Play,
    Stop,
    /// Build: asks Native or Web (the target chooser)
    Build,
    /// the chooser's answers: the native build started from this PC, the native build made
    PlayNative,
    BuildNative,
    /// a build for the browser: `--target web` into the game's web folder
    BuildWeb,
    /// the web build served locally, opened in a Chromium browser
    PlayWeb,
    Properties(usize),
    InstallDlc,
    InstallUpdates,
    RemoveGame,
    InstallKeys,
    OpenKeysFolder,
    ViewMode(i64),
    ToggleFilters,
    ToggleBackground,
    Refresh,
    ViewConsole,
    ToggleConsole,
    About,
    OpenBuildFolder,
    OpenSaveFolder,
    ViewGameLog,
    OpenBuildLog,
    ClearConsole,
    ToolPopup,
    Select(String),
    Activate,
    CardPrimary(String),
    CardProperties(String),
    CardActivate(String),
    /// answers of question boxes
    /// the game's id, and whether its build is deleted too (its saves kept first)
    ConfirmRemove(String, bool),
    ConfirmClose,
}

/// What a command-line run waits for.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Smoke(bool),
    Dlc(bool, String),
    Update(bool, String),
}

/// Why a DLC sync runs: what its end says and does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DlcReason {
    Installed,
    Built,
    Play,
    Changed,
}

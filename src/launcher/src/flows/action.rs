//! Every user command: menus, context menus, shortcuts, card buttons and command-line flags produce
//! these, and `dispatch` handles them (the Qt `QAction`s).

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    AddGame,
    AddFolder,
    ImportBuildPick,
    Settings(usize),
    Exit,
    Play,
    Stop,
    Build,
    Properties(usize),
    InstallDlc,
    InstallUpdates,
    CopySaves,
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
    ConfirmRemove(String),
    ConfirmCopySaves,
    OfferedCopy(String, bool),
    ConfirmClose,
}

/// What a command-line run waits for.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Smoke(bool),
    Dlc(bool, String),
    Update(bool, String),
    SaveCopy(bool, String),
}

/// Why a DLC sync runs: what its end says and does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DlcReason {
    Installed,
    Built,
    Play,
    Changed,
}

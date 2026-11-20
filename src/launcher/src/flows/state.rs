//! The main window's state (the Qt `Window`'s members): the library, the engine runs, what is open.

use super::runs::{BuildRun, DlcRun, ProbeRun};
use super::surface::Surface;
use super::view_state::{BuildProgress, Console, Filters};
use super::{DlcReason, Outcome};
use crate::model::doctor::NxrsDoctor;
use crate::model::facts::Facts;
use crate::model::library::Library;
use crate::services::jobs::Notify;
use crate::services::library_file::Store;
use crate::services::toolchain::Toolchain;
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Receiver;
use std::time::Instant;

pub struct App {
    pub lib: Library,
    pub store: Store,
    pub tools: Toolchain,
    pub tools_checked: bool,
    pub(super) tools_rx: Option<Receiver<Toolchain>>,
    /// the preview library: nothing saved, nothing run
    pub demo: bool,
    /// false for a command-line verb: no dialogs, messages go to stderr
    pub interactive: bool,
    pub selected: String,
    pub running: BTreeMap<String, u32>,
    pub(super) last_poll: Instant,
    /// what the disk says about each game, refreshed after anything that can change it
    pub facts: HashMap<String, Facts>,
    pub(super) build: Option<BuildRun>,
    pub progress: BuildProgress,
    pub(super) probe: Option<ProbeRun>,
    pub(super) dlc_run: Option<DlcRun>,
    pub(super) dlc_reason: DlcReason,
    pub(super) dlc_quiet: bool,
    pub(super) update_quiet: bool,
    pub(super) build_dlc_pending: bool,
    /// closing waits for the cancelled build
    pub closing: bool,
    pub close_now: bool,
    pub console: Console,
    pub job_label: String,
    /// the status bar's bar: None hidden; total 0 = busy
    pub bar: Option<(i64, i64)>,
    pub filters: Filters,
    /// the list's column sort (column, ascending)
    pub table_sort: (usize, bool),
    pub surfaces: Vec<Surface>,
    pub outcomes: Vec<Outcome>,
    pub notify: Notify,
    /// the colour mode and accent shown (a preview or Settings' live choice), saved only by Settings
    pub appearance: (String, String),
    pub(super) doctor_rx: Option<Receiver<NxrsDoctor>>,
    pub doctor_text: String,
    pub doctor_running: bool,
    pub scroll_to_selected: bool,
    /// counts F5 and artwork changes: the UI's image cache is dropped when it moves
    pub artwork_epoch: u64,
}

impl App {
    pub fn new(library_path: &str, demo: bool, notify: Notify) -> App {
        App {
            appearance: ("dark".into(), "lime".into()),
            lib: Library::new(library_path),
            store: Store::default(),
            tools: Toolchain::default(),
            tools_checked: false,
            tools_rx: None,
            demo,
            interactive: true,
            selected: String::new(),
            running: BTreeMap::new(),
            last_poll: Instant::now(),
            facts: HashMap::new(),
            build: None,
            progress: BuildProgress::default(),
            probe: None,
            dlc_run: None,
            dlc_reason: DlcReason::Installed,
            dlc_quiet: false,
            update_quiet: false,
            build_dlc_pending: false,
            closing: false,
            close_now: false,
            console: Console::default(),
            job_label: String::new(),
            bar: None,
            filters: Filters::default(),
            table_sort: (0, true),
            surfaces: Vec::new(),
            outcomes: Vec::new(),
            notify,
            doctor_rx: None,
            doctor_text: String::new(),
            doctor_running: false,
            scroll_to_selected: true,
            artwork_epoch: 0,
        }
    }
}

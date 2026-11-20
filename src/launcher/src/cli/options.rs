//! The command line: the Qt launcher's flags, same names, values and exit codes.

use clap::{Arg, ArgAction, ArgMatches, Command};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub root: Option<String>,
    pub library: Option<String>,
    pub import_build: Option<String>,
    pub demo: bool,
    pub list_view: bool,
    pub show_console: bool,
    pub show_properties: bool,
    pub properties_tab: usize,
    pub show_filters: bool,
    pub preview_theme: Option<String>,
    pub preview_accent: Option<String>,
    pub select_title: Option<String>,
    pub check_image: Option<String>,
    pub show_appearance: bool,
    pub show_controller: bool,
    pub install_dlc: Vec<String>,
    pub remove_dlc: Vec<String>,
    pub install_update: Vec<String>,
    pub remove_update: Vec<String>,
    pub update_choice: Option<String>,
    pub show_updates: bool,
    pub build: bool,
    pub engine: Option<String>,
    pub nxrs_build: Option<String>,
    pub show_dlc: bool,
    pub show_menu: Option<String>,
    pub screenshot: Option<String>,
    pub screenshot_delay: u64,
    pub show_engine_settings: bool,
    pub show_play_settings: bool,
    pub nxrs_doctor: bool,
    pub copy_saves: bool,
    pub smoke_build: Option<String>,
    pub build_output: Option<String>,
}

impl Options {
    /// A command that runs without a window and exits with its outcome.
    pub fn headless(&self) -> bool {
        self.screenshot.is_none()
            && (self.build
                || self.smoke_build.is_some()
                || !self.install_dlc.is_empty()
                || !self.remove_dlc.is_empty()
                || !self.install_update.is_empty()
                || !self.remove_update.is_empty()
                || self.update_choice.is_some()
                || self.copy_saves
                || self.nxrs_doctor)
    }
}

pub fn command() -> Command {
    let flag = |name: &'static str, help: &'static str| {
        Arg::new(name)
            .long(name)
            .help(help)
            .action(ArgAction::SetTrue)
    };
    let value = |name: &'static str, v: &'static str, help: &'static str| {
        Arg::new(name)
            .long(name)
            .value_name(v)
            .help(help)
            .num_args(1)
    };
    let many = |name: &'static str, v: &'static str, help: &'static str| {
        Arg::new(name)
            .long(name)
            .value_name(v)
            .help(help)
            .action(ArgAction::Append)
    };
    Command::new("nxrs")
        .about("nxrs — native game library and build launcher")
        .version(env!("NXRS_VERSION"))
        .arg(value("root", "directory", "Kept for older scripts (the Turbo engine folder); has no effect"))
        .arg(value("library", "file", "Use a specific library file"))
        .arg(value("import-build", "exe", "Import an existing native game executable"))
        .arg(flag("demo", "Show a non-persistent preview library"))
        .arg(flag("list-view", "Start in the details-list view"))
        .arg(flag("show-console", "Show the build-output dock"))
        .arg(flag("show-properties", "Open the selected game's Properties dialog"))
        .arg(flag("show-filters", "Open the library search and filter popout"))
        .arg(value("preview-theme", "mode", "Preview dark or light mode without saving"))
        .arg(value("preview-accent", "color", "Preview an accent without saving"))
        .arg(value("select-title", "title", "Select a library game by display title"))
        .arg(value("check-image", "file", "Validate an artwork image"))
        .arg(flag("show-appearance", "Open appearance settings"))
        .arg(flag("show-controller", "Open controller settings"))
        .arg(value("properties-tab", "index", "Tab for --show-properties (0 General, 1 Build, 2 Runtime, 3 Artwork, 4 DLC, 5 Updates)"))
        .arg(many("install-dlc", "package", "Install a DLC package for the --select-title game and exit (repeatable)"))
        .arg(many("remove-dlc", "package", "Remove a DLC package from the --select-title game and exit (repeatable)"))
        .arg(many("install-update", "package", "Add an update package to the --select-title game and exit (repeatable)"))
        .arg(many("remove-update", "package", "Remove an update package from the --select-title game and exit (repeatable)"))
        .arg(value("update-choice", "choice", "Version to build for the --select-title game: newest, none or an installed package"))
        .arg(flag("show-updates", "Open the selected game's Updates window (Game > Install Updates…)"))
        .arg(flag("build", "Build the --select-title game with its saved options and exit"))
        .arg(value("engine", "engine", "Engine of the --select-title game, saved: nxrs (before --build and the DLC/update options)"))
        .arg(value("nxrs-build", "exe", "nxrs-build to use instead of the bundled one, saved in the library (empty: the bundled one)"))
        .arg(flag("show-dlc", "Open the selected game's DLC window (Game > Install DLC…)"))
        .arg(value("show-menu", "name", "Open a menu of the title bar (File, Game, Keys, View, Help)"))
        .arg(value("screenshot", "png", "Save a launcher preview and exit"))
        .arg(value("screenshot-delay", "ms", "Milliseconds before --screenshot grabs the window (default 1100)"))
        .arg(flag("show-engine-settings", "Open Settings > Engine (nxrs-build doctor runs there)"))
        .arg(flag("show-play-settings", "Open Settings > Play (screen mode, background pause and mute: every game)"))
        .arg(flag("nxrs-doctor", "Run nxrs-build doctor for the configured nxrs-build, print its checks and exit (0 = all ok)"))
        .arg(flag("copy-saves", "Copy the --select-title game's saves from its Turbo build into its nxrs build (backup first) and exit"))
        .arg(value("smoke-build", "input", "Build a synthetic test input and exit (diagnostics)"))
        .arg(value("build-output", "directory", "Output folder for --smoke-build"))
}

pub fn parse_from<I, T>(args: I) -> Result<Options, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let m = command().try_get_matches_from(args)?;
    Ok(from_matches(&m))
}

fn from_matches(m: &ArgMatches) -> Options {
    let s = |n: &str| m.get_one::<String>(n).cloned();
    let v = |n: &str| {
        m.get_many::<String>(n)
            .map(|x| x.cloned().collect())
            .unwrap_or_default()
    };
    let f = |n: &str| m.get_flag(n);
    Options {
        root: s("root"),
        library: s("library"),
        import_build: s("import-build"),
        demo: f("demo"),
        list_view: f("list-view"),
        show_console: f("show-console"),
        show_properties: f("show-properties"),
        properties_tab: s("properties-tab")
            .and_then(|t| t.parse().ok())
            .unwrap_or(0),
        show_filters: f("show-filters"),
        preview_theme: s("preview-theme"),
        preview_accent: s("preview-accent"),
        select_title: s("select-title"),
        check_image: s("check-image"),
        show_appearance: f("show-appearance"),
        show_controller: f("show-controller"),
        install_dlc: v("install-dlc"),
        remove_dlc: v("remove-dlc"),
        install_update: v("install-update"),
        remove_update: v("remove-update"),
        update_choice: s("update-choice"),
        show_updates: f("show-updates"),
        build: f("build"),
        engine: s("engine"),
        nxrs_build: s("nxrs-build"),
        show_dlc: f("show-dlc"),
        show_menu: s("show-menu"),
        screenshot: s("screenshot"),
        screenshot_delay: s("screenshot-delay")
            .and_then(|t| t.parse().ok())
            .unwrap_or(1100),
        show_engine_settings: f("show-engine-settings"),
        show_play_settings: f("show-play-settings"),
        nxrs_doctor: f("nxrs-doctor"),
        copy_saves: f("copy-saves"),
        smoke_build: s("smoke-build"),
        build_output: s("build-output"),
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;

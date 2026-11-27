//! The command line: the Qt launcher's flags, same names, values and exit codes.

use clap::ArgMatches;

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
    pub source: Option<String>,
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
    pub build_web: bool,
    pub serve_web: Option<String>,
    pub engine: Option<String>,
    pub nxrs_build: Option<String>,
    pub show_dlc: bool,
    pub show_menu: Option<String>,
    pub screenshot: Option<String>,
    pub screenshot_delay: u64,
    pub show_engine_settings: bool,
    pub show_play_settings: bool,
    pub nxrs_doctor: bool,
    pub smoke_build: Option<String>,
    pub build_output: Option<String>,
    pub migrate_builds: bool,
    pub show_window: Option<String>,
    pub show_add: Option<String>,
}

impl Options {
    /// A command that runs without a window and exits with its outcome.
    pub fn headless(&self) -> bool {
        self.screenshot.is_none()
            && (self.build
                || self.build_web
                || self.smoke_build.is_some()
                || !self.install_dlc.is_empty()
                || !self.remove_dlc.is_empty()
                || !self.install_update.is_empty()
                || !self.remove_update.is_empty()
                || self.update_choice.is_some()
                || self.source.is_some()
                || self.nxrs_doctor
                || self.migrate_builds)
    }
}

pub use super::flags::command;

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
        source: s("source"),
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
        build_web: f("build-web"),
        serve_web: s("serve-web"),
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
        smoke_build: s("smoke-build"),
        build_output: s("build-output"),
        migrate_builds: f("migrate-builds"),
        show_window: s("show-window"),
        show_add: s("show-add"),
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;

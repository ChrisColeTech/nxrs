//! What the flags do to a freshly opened window or headless run, in the Qt launcher's order: import,
//! nxrs-build, select, engine, appearance preview, the views and windows to open.

use super::Options;
use crate::flows::{Action, App};

/// Before anything runs. Err(2): `--engine` refused.
pub fn apply(app: &mut App, o: &Options) -> Result<(), i32> {
    if let Some(exe) = &o.import_build {
        app.import_build(exe);
    }
    if let Some(path) = &o.nxrs_build {
        app.set_nxrs_build(path);
    }
    if let Some(title) = &o.select_title {
        app.select_title(title);
    }
    if let Some(engine) = &o.engine
        && !app.set_engine(engine)
    {
        eprintln!(
            "Cannot set the engine: select a game that is not running or building, and use nxrs (the only engine)"
        );
        return Err(2);
    }
    if o.preview_theme.is_some() || o.preview_accent.is_some() {
        let mode = o.preview_theme.clone().unwrap_or_else(|| "dark".into());
        app.appearance = (
            mode,
            o.preview_accent.clone().unwrap_or_else(|| "lime".into()),
        );
    }
    if o.list_view {
        app.lib.view_mode = 1;
    }
    if o.show_console {
        app.lib.console.visible = true;
    }
    Ok(())
}

/// The windows the flags open (after the window is up).
pub fn windows(app: &mut App, o: &Options) {
    let opens = [
        (o.show_properties, Action::Properties(o.properties_tab)),
        (o.show_updates, Action::InstallUpdates),
        (o.show_dlc, Action::InstallDlc),
        (o.show_appearance, Action::Settings(3)),
        (o.show_controller, Action::Settings(2)),
        (o.show_engine_settings, Action::Settings(0)),
        (o.show_play_settings, Action::Settings(4)),
    ];
    for (wanted, action) in opens {
        if wanted {
            app.dispatch(action);
        }
    }
    let named = match o.show_window.as_deref() {
        Some("remove") => Some(Action::RemoveGame),
        Some("about") => Some(Action::About),
        Some("build-settings") => Some(Action::Settings(1)),
        Some("tool") => Some(Action::ToolPopup),
        Some("close-build") => {
            app.ask_close();
            None
        }
        Some("play") => Some(Action::Play),
        Some("build") => Some(Action::Build),
        _ => None,
    };
    named.into_iter().for_each(|a| app.dispatch(a));
    if let Some(source) = &o.show_add {
        app.import_source(source, true, "");
    }
    if o.show_filters {
        app.filters.open = true;
        app.filters.focus_search = true;
    }
}

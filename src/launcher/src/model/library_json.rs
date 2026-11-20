//! The library file's JSON: read with the Qt launcher's defaults and clamps, written with every key it
//! wrote plus this launcher's own (`window`, `consolePanel`); keys neither knows are kept.

use super::game_json;
use super::json_fields::{bool_of, f32_of, int_of, str_of, str_or};
use super::library::{ConsolePanel, Library, WindowPlacement};
use super::play::normalize_button_layout;
use base64::Engine;
use serde_json::{Map, Value, json};

const KNOWN: &[&str] = &[
    "schema",
    "games",
    "toolRoot",
    "toolRootChosen",
    "outputRoot",
    "keysPath",
    "nxrsBuild",
    "jobs",
    "viewMode",
    "coverWidth",
    "showBackground",
    "colorMode",
    "accentColor",
    "buttonLayout",
    "pauseOnFocusLoss",
    "muteOnFocusLoss",
    "docked",
    "askUser",
    "geometry",
    "dockState",
    "window",
    "consolePanel",
];
/// A key only this launcher writes: its absence says the Qt launcher wrote the file last.
pub const OWN_KEY: &str = "consolePanel";

/// The settings of a schema-1 document (`o`); the games are read by the caller, which checks them
/// against the disk.
pub fn settings_from(o: &Map<String, Value>, lib: &mut Library) {
    let b64 = base64::engine::general_purpose::STANDARD;
    lib.tool_root = str_of(o, "toolRoot");
    lib.tool_root_chosen = bool_of(o, "toolRootChosen", false);
    lib.output_root = str_of(o, "outputRoot");
    lib.keys_path = str_of(o, "keysPath");
    lib.nxrs_build = str_of(o, "nxrsBuild");
    lib.jobs = int_of(o, "jobs", 8).clamp(1, 64);
    lib.view_mode = int_of(o, "viewMode", 0).clamp(0, 1);
    lib.cover_width = int_of(o, "coverWidth", 180).clamp(130, 240);
    lib.show_background = bool_of(o, "showBackground", true);
    let mode = str_or(o, "colorMode", "dark");
    lib.color_mode = if mode == "light" { mode } else { "dark".into() };
    lib.accent_color = str_or(o, "accentColor", "lime");
    lib.button_layout = normalize_button_layout(&str_of(o, "buttonLayout"));
    if lib.button_layout.is_empty() {
        lib.button_layout = "switch".into();
    }
    lib.pause_on_focus_loss = bool_of(o, "pauseOnFocusLoss", false);
    lib.mute_on_focus_loss = bool_of(o, "muteOnFocusLoss", false);
    lib.docked = bool_of(o, "docked", true);
    lib.ask_user = bool_of(o, "askUser", false);
    lib.geometry = b64.decode(str_of(o, "geometry")).unwrap_or_default();
    lib.dock_state = b64.decode(str_of(o, "dockState")).unwrap_or_default();
    lib.window = o
        .get("window")
        .and_then(Value::as_object)
        .map(|w| WindowPlacement {
            x: f32_of(w, "x", 0.0),
            y: f32_of(w, "y", 0.0),
            width: f32_of(w, "width", 1280.0),
            height: f32_of(w, "height", 800.0),
            maximized: bool_of(w, "maximized", false),
        });
    if let Some(c) = o.get(OWN_KEY).and_then(Value::as_object) {
        lib.console = ConsolePanel {
            visible: bool_of(c, "visible", false),
            right: str_of(c, "side") == "right",
            floating: bool_of(c, "floating", false),
            size: f32_of(c, "size", crate::constants::layout::CONSOLE_DEFAULT),
        };
    }
    lib.extra = o
        .iter()
        .filter(|(k, _)| !KNOWN.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
}

pub fn to_json(lib: &Library) -> Value {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut o = lib.extra.clone();
    let side = if lib.console.right { "right" } else { "bottom" };
    let known = json!({
        "schema": 1, "games": lib.games.iter().map(game_json::to_json).collect::<Vec<_>>(),
        "toolRoot": lib.tool_root, "toolRootChosen": lib.tool_root_chosen,
        "outputRoot": lib.output_root, "keysPath": lib.keys_path, "nxrsBuild": lib.nxrs_build,
        "jobs": lib.jobs, "viewMode": lib.view_mode, "coverWidth": lib.cover_width,
        "showBackground": lib.show_background, "colorMode": lib.color_mode,
        "accentColor": lib.accent_color, "buttonLayout": lib.button_layout,
        "pauseOnFocusLoss": lib.pause_on_focus_loss, "muteOnFocusLoss": lib.mute_on_focus_loss,
        "docked": lib.docked, "askUser": lib.ask_user,
        "geometry": b64.encode(&lib.geometry), "dockState": b64.encode(&lib.dock_state),
        "consolePanel": {"visible": lib.console.visible, "side": side, "floating": lib.console.floating, "size": lib.console.size.round() as i64},
    });
    if let Value::Object(map) = known {
        o.extend(map);
    }
    if let Some(w) = &lib.window {
        let place = json!({"x": w.x.round() as i64, "y": w.y.round() as i64, "width": w.width.round() as i64,
            "height": w.height.round() as i64, "maximized": w.maximized});
        o.insert("window".into(), place);
    }
    Value::Object(o)
}

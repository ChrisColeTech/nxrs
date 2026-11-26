//! A game in the library file: the keys the Qt launcher wrote, read with its defaults and clamps; keys
//! this launcher does not know are kept and written back.

use super::build_layout::nxrs_folder_for;
use super::dates;
use super::game::Game;
use super::game_update::GameUpdate;
use super::json_fields::{bool_of, int_of, paths_of, str_of};
use super::path_text::same_path;
use super::play::{normalize_button_layout, normalize_screen_mode};
use crate::constants::engine as e;
use serde_json::{Map, Value, json};

const KNOWN: &[&str] = &[
    "id",
    "title",
    "source",
    "buildDir",
    "outputName",
    "cover",
    "background",
    "renderer",
    "buttonLayout",
    "screenMode",
    "workers",
    "optimization",
    "builtSource",
    "buildIncomplete",
    "lastPlayed",
    "lastBuilt",
    "dlc",
    "updates",
    "updateChoice",
    "builtUpdate",
    "engine",
    "hintsBuilt",
    "savesOffered",
    // a per-game pauseOnFocusLoss (written by a launcher of 382ff33) is not read: the setting is
    // global, and the next save leaves it out
    "pauseOnFocusLoss",
];

pub fn to_json(g: &Game) -> Value {
    let mut o = g.extra.clone();
    let updates: Vec<Value> = g
        .updates
        .iter()
        .map(|u| json!({"path": u.path, "titleId": u.title_id, "version": u.version, "display": u.display}))
        .collect();
    let known = json!({
        "id": g.id, "title": g.title, "source": g.source, "buildDir": g.build_dir,
        "outputName": g.output_name, "cover": g.cover, "background": g.background,
        "renderer": g.renderer, "buttonLayout": g.button_layout, "screenMode": g.screen_mode,
        "workers": g.workers, "optimization": g.optimization, "builtSource": g.built_source,
        "buildIncomplete": g.build_incomplete, "lastPlayed": dates::format(&g.last_played),
        "lastBuilt": dates::format(&g.last_built), "dlc": g.dlc, "updates": updates,
        "updateChoice": g.update_choice, "builtUpdate": g.built_update, "engine": g.engine,
        "hintsBuilt": g.hints_built, "savesOffered": g.saves_offered,
    });
    if let Value::Object(map) = known {
        o.extend(map);
    }
    Value::Object(o)
}

fn update_of(value: &Value) -> GameUpdate {
    let empty = Map::new();
    let e = value.as_object().unwrap_or(&empty);
    GameUpdate {
        path: str_of(e, "path"),
        title_id: str_of(e, "titleId"),
        version: int_of(e, "version", 0),
        display: str_of(e, "display"),
    }
}

pub fn from_json(value: &Value) -> Game {
    let empty = Map::new();
    let o = value.as_object().unwrap_or(&empty);
    let renderer = match o.get("renderer") {
        Some(Value::String(s)) if s == "vulkan" || s == "opengl" => s.clone(),
        Some(_) => "opengl".into(),
        None => "vulkan".into(),
    };
    let mut updates: Vec<GameUpdate> = Vec::new();
    if let Some(Value::Array(list)) = o.get("updates") {
        for u in list.iter().map(update_of) {
            if !u.path.is_empty() && !updates.iter().any(|h| same_path(&h.path, &u.path)) {
                updates.push(u);
            }
        }
    }
    let mut g = Game {
        id: str_of(o, "id"),
        title: str_of(o, "title"),
        source: str_of(o, "source"),
        build_dir: str_of(o, "buildDir"),
        output_name: str_of(o, "outputName"),
        cover: str_of(o, "cover"),
        background: str_of(o, "background"),
        renderer,
        engine: e::ENGINE_NXRS.into(),
        button_layout: normalize_button_layout(&str_of(o, "buttonLayout")),
        screen_mode: normalize_screen_mode(&str_of(o, "screenMode")),
        built_source: str_of(o, "builtSource"),
        dlc: paths_of(o, "dlc"),
        updates,
        update_choice: str_of(o, "updateChoice"),
        built_update: str_of(o, "builtUpdate"),
        build_incomplete: bool_of(o, "buildIncomplete", false),
        workers: int_of(o, "workers", 0).clamp(0, 64),
        optimization: int_of(o, "optimization", 2).clamp(0, 3),
        last_played: dates::parse(&str_of(o, "lastPlayed")),
        last_built: dates::parse(&str_of(o, "lastBuilt")),
        hints_built: int_of(o, "hintsBuilt", 0).max(0),
        saves_offered: str_of(o, "savesOffered"),
        extra: o
            .iter()
            .filter(|(k, _)| !KNOWN.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    };
    // an entry saved with another engine (or none) loads as nxrs, in an nxrs folder and not built
    if str_of(o, "engine") != e::ENGINE_NXRS && !g.build_dir.is_empty() {
        g.build_dir = nxrs_folder_for(&g.build_dir);
        g.built_source.clear();
        g.built_update.clear();
        g.build_incomplete = false;
    }
    g
}

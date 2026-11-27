//! Fixtures the integration tests share: a temporary folder, files, a built game, a launcher state whose
//! nxrs-build is the fake engine.
#![allow(dead_code)]

use nxrs_launcher::flows::App;
use nxrs_launcher::model::game::Game;
use nxrs_launcher::model::path_text::from_native;
use nxrs_launcher::services::jobs::noop;
use nxrs_launcher::services::toolchain::Toolchain;
use serde_json::{Value, json};
use std::process::{Command, Output};
use std::time::Duration;

pub const FAKE: &str = env!("CARGO_BIN_EXE_fake-engine");
pub const NXRS: &str = env!("CARGO_BIN_EXE_nxrs");

pub fn tmp() -> (tempfile::TempDir, String) {
    let t = tempfile::Builder::new()
        .prefix("nxrs launcher [test] ")
        .tempdir()
        .unwrap();
    let p = from_native(&t.path().to_string_lossy());
    (t, p)
}
pub fn write(path: &str, bytes: &[u8]) {
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
pub fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}
/// A built nxrs game: executable, code module, data folder, its source package.
pub fn built_game(dir: &str) -> Game {
    let g = Game {
        id: "g".into(),
        title: "Kart".into(),
        output_name: "Kart".into(),
        build_dir: format!("{dir}/Build output [nxrs]"),
        engine: "nxrs".into(),
        source: format!("{dir}/Kart.nsp"),
        built_source: format!("{dir}/Kart.nsp"),
        ..Game::default()
    };
    std::fs::create_dir_all(g.data_dir()).unwrap();
    write(&g.executable(), b"fixture");
    write(&g.code_module(), b"fixture");
    write(&g.source, b"PFS0");
    g
}
/// A launcher state over `dir/library.json` holding `games`, its nxrs-build the fake engine and its
/// keys installed.
pub fn app_with(dir: &str, games: Vec<Game>) -> App {
    let mut app = App::new(&format!("{dir}/library.json"), false, noop());
    app.interactive = false;
    app.open();
    app.lib.games = games;
    let keys = format!("{dir}/keys/prod.keys");
    write(&keys, b"header_key = 00");
    app.tools = Toolchain {
        nxrs_build: from_native(FAKE),
        nxrs_version: String::new(),
        nxrs_contract: 2,
        keys,
        browser: None,
    };
    app.tools_checked = true;
    app.refresh_facts();
    if let Some(g) = app.lib.games.first() {
        app.selected = g.id.clone();
    }
    app
}
pub fn settle(app: &mut App) {
    assert!(
        app.wait_idle(Duration::from_secs(30)),
        "the engine run did not end"
    );
}

/// Runs nxrs with its own data folder: never the owner's.
pub fn nxrs(data: &str, args: &[&str]) -> Output {
    Command::new(NXRS)
        .args(args)
        .env("NXRS_DATA_DIR", data)
        .env_remove("NXRS_BUILD")
        .output()
        .unwrap()
}

pub fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

pub fn library(path: &str) -> Value {
    serde_json::from_str(&read(path)).unwrap()
}

/// A library holding one built nxrs game "Kart" (its package exists, keys installed in the data folder).
pub fn fixture(dir: &str) -> String {
    let g = built_game(dir);
    write(&format!("{dir}/data/keys/prod.keys"), b"header_key = 00");
    let lib = format!("{dir}/library.json");
    let entry = json!({"id": "g", "title": "Kart", "source": g.source, "builtSource": g.source, "buildDir": g.build_dir, "outputName": "Kart", "engine": "nxrs"});
    write(
        &lib,
        json!({"schema": 1, "games": [entry], "nxrsBuild": FAKE})
            .to_string()
            .as_bytes(),
    );
    lib
}

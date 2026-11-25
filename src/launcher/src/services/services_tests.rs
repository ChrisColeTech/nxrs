//! Fixtures the services' tests share, and an entry saved with another engine loading as nxrs.

use super::build_facts::has_build;
use crate::constants::engine as e;
use crate::model::game::Game;
use crate::model::game_json;
use crate::model::path_text::from_native;

pub fn tmp() -> (tempfile::TempDir, String) {
    let t = tempfile::tempdir().unwrap();
    let p = from_native(&t.path().to_string_lossy());
    (t, p)
}
pub fn write(path: &str, bytes: &[u8]) {
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
pub fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|_| b"<missing>".to_vec())
}
pub fn contains(bytes: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(bytes).contains(needle)
}
/// A built nxrs game on disk: executable, code module, data folder with exefs/.
pub fn built_game(dir: &str) -> Game {
    let g = Game {
        id: "g".into(),
        title: "Kart".into(),
        output_name: "Kart".into(),
        build_dir: format!("{dir}/Build output"),
        engine: e::ENGINE_NXRS.into(),
        ..Game::default()
    };
    std::fs::create_dir_all(format!("{}/exefs", g.data_dir())).unwrap();
    write(&g.executable(), b"fixture");
    write(&g.code_module(), b"fixture");
    g
}

/// An entry an older engine built (`"engine": "turbo"`, or no engine) loads as nxrs, moved to its
/// `[nxrs]` folder and not built, even with the older build still on disk; it saves back as nxrs, and
/// `savesOffered` is kept as it was.
#[test]
fn other_engine_entries_load_as_nxrs() {
    let (_t, dir) = tmp();
    let old = built_game(&dir);
    for engine in [serde_json::json!("turbo"), serde_json::Value::Null] {
        let mut v = game_json::to_json(&old);
        v["engine"] = engine;
        v["builtSource"] = serde_json::json!("D:/x.nsp");
        v["savesOffered"] = serde_json::json!("offered");
        let g = game_json::from_json(&v);
        assert_eq!(g.engine, e::ENGINE_NXRS);
        assert_eq!(g.build_dir, format!("{dir}/Build output [nxrs]"));
        assert!(g.nxrs() && g.built_source.is_empty() && !has_build(&g));
        assert_eq!(g.saves_offered, "offered");
        let back = game_json::to_json(&g);
        assert_eq!(back["engine"], e::ENGINE_NXRS);
        assert_eq!(game_json::from_json(&back).build_dir, g.build_dir);
    }
    assert!(has_build(&game_json::from_json(&game_json::to_json(&old))));
    assert!(contains(&read(&old.executable()), "fixture"));
}

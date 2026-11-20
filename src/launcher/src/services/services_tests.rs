//! Fixtures the services' tests share, and the save import (`savesCopyBetweenEngines`, Turbo to nxrs).

use super::markers::set_game_engine;
use super::saves::{copy_from_turbo, turbo_build_has_saves};
use super::sources::game_from_source;
use crate::constants::engine as e;
use crate::model::game::Game;
use crate::model::game_json;
use crate::model::names::build_folder_for;
use crate::model::path_text::from_native;
use crate::model::save_plan::save_folder;

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

#[test]
fn saves_copy_from_turbo() {
    let (_t, dir) = tmp();
    let source = format!("{dir}/Kart.nsp");
    write(&source, b"PFS0");
    let mut game = game_from_source(&source, "");
    game.engine = e::ENGINE_TURBO.into();
    game.build_dir = format!("{dir}/builds/Kart");
    game.output_name = "Kart".into();
    let turbo_save = save_folder(&game.build_dir, "Kart");
    let nxrs_dir = build_folder_for(&game.build_dir, true);
    let nxrs_save = save_folder(&nxrs_dir, "Kart");
    write(
        &format!("{turbo_save}/user/progress.dat"),
        b"turbo progress",
    );
    write(
        &format!("{turbo_save}/user/sub/options.bin"),
        b"turbo options",
    );
    write(&format!("{turbo_save}/device/device.bin"), b"turbo device");
    write(&format!("{turbo_save}/temp/scratch"), b"temporary");
    write(&format!("{turbo_save}/bcat/news"), b"bcat");
    write(&format!("{nxrs_save}/user1/progress.dat"), b"nxrs progress");
    write(
        &format!("{nxrs_save}/user1.new/progress.dat"),
        b"half-committed",
    );
    assert!(
        !turbo_build_has_saves(&game),
        "a Turbo entry has nothing to import into"
    );
    assert!(set_game_engine(&mut game, true));
    assert_eq!(game.build_dir, nxrs_dir);
    assert!(turbo_build_has_saves(&game));
    let copy = copy_from_turbo(&game).unwrap();
    assert_eq!(
        read(&format!("{nxrs_save}/user1/progress.dat")),
        b"turbo progress"
    );
    assert_eq!(
        read(&format!("{nxrs_save}/user1/sub/options.bin")),
        b"turbo options"
    );
    assert_eq!(
        read(&format!("{nxrs_save}/device/device.bin")),
        b"turbo device"
    );
    for gone in ["temp", "bcat", "user1.new"] {
        assert!(
            !std::path::Path::new(&format!("{nxrs_save}/{gone}")).exists(),
            "{gone}"
        );
    }
    assert_eq!(
        read(&format!("{}/user1/progress.dat", copy.backup)),
        b"nxrs progress"
    );
    assert_eq!(
        read(&format!("{}/user1.new/progress.dat", copy.backup)),
        b"half-committed"
    );
    assert_eq!(
        read(&format!("{turbo_save}/user/progress.dat")),
        b"turbo progress"
    );
    assert!(copy.report.contains("user1"));
    // Turbo's runtimes before the split kept the user saves in save/system
    let (_t2, old) = tmp();
    let mut legacy = game_from_source(&source, "");
    legacy.engine = e::ENGINE_TURBO.into();
    legacy.build_dir = format!("{old}/Kart");
    legacy.output_name = "Kart".into();
    write(
        &format!("{}/system/slot0", save_folder(&legacy.build_dir, "Kart")),
        b"old turbo",
    );
    assert!(set_game_engine(&mut legacy, true));
    assert!(
        copy_from_turbo(&legacy)
            .unwrap_err()
            .contains("Build the game")
    );
    std::fs::create_dir_all(legacy.data_dir()).unwrap();
    let copy = copy_from_turbo(&legacy).unwrap();
    assert!(copy.backup.is_empty());
    assert_eq!(
        read(&format!(
            "{}/user1/slot0",
            save_folder(&legacy.build_dir, "Kart")
        )),
        b"old turbo"
    );
    legacy.saves_offered = e::ENGINE_TURBO.into();
    assert_eq!(
        game_json::from_json(&game_json::to_json(&legacy)).saves_offered,
        e::ENGINE_TURBO
    );
}

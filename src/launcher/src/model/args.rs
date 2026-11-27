//! nxrs-build's arguments (launcher contract v2): a build, an update check, a DLC sync. `keys` is
//! passed only when the installed file exists (`keys_exist`).

use super::game::Game;
use super::path_text::native;

fn keys_args(args: &mut Vec<String>, keys: &str, keys_exist: bool) {
    if !keys.is_empty() && keys_exist {
        args.push("--keys".into());
        args.push(native(keys));
    }
}

/// `build <src> --outdir <dir> -o <name> --jobs <n> --machine [--no-update | --update <pkg>]
/// [--keys <file>]`; the game's own workers over the launcher default, 1 to 64.
pub fn build(game: &Game, keys: &str, keys_exist: bool, jobs: i64) -> Vec<String> {
    let n = if game.workers != 0 {
        game.workers
    } else {
        jobs
    }
    .clamp(1, 64);
    let mut args: Vec<String> = vec![
        "build".into(),
        native(&game.source),
        "--outdir".into(),
        native(&game.build_dir),
        "-o".into(),
        game.output_name.clone(),
        "--jobs".into(),
        n.to_string(),
        "--machine".into(),
    ];
    let update = game.update_token();
    if update == "none" {
        args.push("--no-update".into());
    } else if !update.is_empty() {
        args.push("--update".into());
        args.push(native(&update));
    }
    keys_args(&mut args, keys, keys_exist);
    args
}

/// The same build for the web (docs/35 W5): `--outdir <web folder> … --target web`.
pub fn build_web(game: &Game, keys: &str, keys_exist: bool, jobs: i64) -> Vec<String> {
    let mut web = game.clone();
    web.build_dir = super::web_build::web_dir(game);
    let mut args = build(&web, keys, keys_exist, jobs);
    if !game.build_dir.is_empty() {
        args.push(crate::constants::web::SHADERS_FROM_ARG.into());
        args.push(native(&game.data_dir()));
    }
    args.extend(crate::constants::web::TARGET_ARGS.map(String::from));
    args
}

/// `update-info --game <src> [--keys <file>] <pkg>...`
pub fn update_info(game: &Game, keys: &str, keys_exist: bool, packages: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec!["update-info".into(), "--game".into(), native(&game.source)];
    keys_args(&mut args, keys, keys_exist);
    args.extend(packages.iter().map(|p| native(p)));
    args
}

/// `dlc sync --datadir <data> [--keys <file>] <pkg>...`
pub fn dlc_sync(game: &Game, keys: &str, keys_exist: bool) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "dlc".into(),
        "sync".into(),
        "--datadir".into(),
        native(&game.data_dir()),
    ];
    keys_args(&mut args, keys, keys_exist);
    args.extend(game.dlc.iter().map(|p| native(p)));
    args
}

pub const DOCTOR: &str = "doctor";
pub const CONTRACT: &str = "contract";

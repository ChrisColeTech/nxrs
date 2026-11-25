//! What can be added to the library: a source package or folder, or a native build on disk.

use super::files::{is_dir, is_file, read_at};
use super::markers::{adopt_built_update, build_cover, source_of_build};
use crate::constants::{engine as e, words_library as w};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::names::{build_folder_for, output_name_for, title_from_name};
use crate::model::path_text::{
    absolute, base_name, ends_with_ci, file_name, from_native, join, parent, suffix,
};

fn at(bytes: &[u8], i: usize, what: &[u8]) -> bool {
    bytes.get(i..i + what.len()) == Some(what)
}

/// An extracted folder (`main`, `exefs/main`, `ExeFS/main`), an indexed NUP, an application NSP, a
/// card image (XCI) or an NRO.
pub fn valid_source(source: &str) -> Result<(), String> {
    if is_dir(source) {
        if ["main", "exefs/main", "ExeFS/main"]
            .iter()
            .any(|n| is_file(&join(source, n)))
        {
            return Ok(());
        }
        return Err(w::NO_MAIN.into());
    }
    let head = read_at(source, 0, 0x1104)?;
    let ok = match suffix(source).as_str() {
        "nup" => head.starts_with(b"NUP\x01"),
        "nsp" => head.starts_with(b"PFS0"),
        "nro" => at(&head, 0x10, b"NRO0"),
        // a card image: "HEAD" at 0x100, after the 0x1000-byte key area when the dump kept it
        "xci" if at(&head, 0x100, b"HEAD") || at(&head, 0x1100, b"HEAD") => true,
        "xci" => return Err(fill(w::NOT_CARD, &[&file_name(source)])),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(w::NOT_SOURCE.into())
    }
}

/// The title of a source: its folder or file name, tags removed.
pub fn title_for_source(source: &str) -> String {
    title_from_name(&if is_dir(source) {
        file_name(source)
    } else {
        base_name(source)
    })
}

/// A new entry for a source: the nxrs engine's, built into `<build location>\<name> [nxrs]` (the
/// default output folder, else `SwitchRecomp Builds` beside the source).
pub fn game_from_source(source: &str, output_root: &str) -> Game {
    let title = title_for_source(source);
    let output_name = output_name_for(&title);
    let base = if output_root.is_empty() {
        join(&parent(source), e::DEFAULT_BUILDS)
    } else {
        from_native(output_root)
    };
    Game {
        id: uuid::Uuid::new_v4().to_string(),
        source: absolute(source),
        build_dir: build_folder_for(&join(&base, &output_name)),
        engine: e::ENGINE_NXRS.into(),
        title,
        output_name,
        ..Game::default()
    }
}

/// A native build on disk: its executable, code module and data folder; source, update, renderer and
/// cover read from what is there.
pub fn game_from_executable(path: &str) -> Result<Game, String> {
    if !is_file(path) {
        return Err(w::NO_EXECUTABLE.into());
    }
    let mut title = file_name(path);
    if !e::EXECUTABLE_SUFFIX.is_empty() && ends_with_ci(&title, e::EXECUTABLE_SUFFIX) {
        title.truncate(title.len() - e::EXECUTABLE_SUFFIX.len());
    }
    let mut g = Game {
        id: uuid::Uuid::new_v4().to_string(),
        output_name: title.clone(),
        title,
        build_dir: parent(path),
        ..Game::default()
    };
    if !super::build_facts::has_build(&g) {
        return Err(w::INCOMPLETE_BUILD.into());
    }
    g.source = source_of_build(&g.data_dir());
    adopt_built_update(&mut g);
    g.built_source = g.source.clone();
    g.renderer = super::ini_file::read_renderer(&g.ini_path());
    g.cover = build_cover(&g.data_dir());
    Ok(g)
}

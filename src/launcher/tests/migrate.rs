//! `nxrs --migrate-builds` (owner, 2026-11-26): build folders of the old layout move to
//! `<source folder>\nxrs\<name>` and the library follows, in its Qt format, backed up first; a folder
//! in use or whose new place is taken stays and is reported; other engines' folders and a non-empty
//! `SwitchRecomp Builds` stay.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;

fn entry(id: &str, title: &str, dir: &str) -> serde_json::Value {
    json!({"id": id, "title": title, "outputName": title, "engine": "nxrs",
        "source": format!("{dir}/{title}.nsp"), "builtSource": format!("{dir}/{title}.nsp"),
        "buildDir": format!("{dir}/SwitchRecomp Builds/{title} [nxrs]")})
}

fn build(dir: &str, title: &str) {
    let b = format!("{dir}/SwitchRecomp Builds/{title} [nxrs]");
    write(&format!("{b}/{title}.exe"), b"exe");
    write(&format!("{b}/{title}_data/save/slot"), b"save");
}

#[test]
fn old_layout_folders_move_and_the_library_follows() {
    let (_t, dir) = tmp();
    let (a, b) = (format!("{dir}/A"), format!("{dir}/B"));
    // A: moved; the old engine's build beside it keeps SwitchRecomp Builds in place
    build(&a, "Kart");
    write(
        &format!("{a}/SwitchRecomp Builds/Kart/Kart.exe"),
        b"old engine",
    );
    // B: Party moves, Taken's new place exists (B's SwitchRecomp Builds stays), Unbuilt has no folder
    build(&b, "Party");
    build(&b, "Taken");
    write(&format!("{b}/nxrs/Taken/keep.txt"), b"x");
    // C: Solo moves and its SwitchRecomp Builds, left empty, goes
    let c = format!("{dir}/C");
    build(&c, "Solo");
    let lib = format!("{dir}/library.json");
    let games = [
        entry("1", "Kart", &a),
        entry("2", "Party", &b),
        entry("3", "Taken", &b),
        entry("4", "Unbuilt", &b),
        entry("5", "Solo", &c),
    ];
    write(
        &lib,
        json!({"schema": 1, "games": games}).to_string().as_bytes(),
    );
    let before = read(&lib);
    let out = nxrs(
        &format!("{dir}/data"),
        &["--library", &lib, "--migrate-builds"],
    );
    let said = text(&out);
    assert_eq!(out.status.code(), Some(1), "one entry was left: {said}");
    assert!(
        said.contains("Moved: Kart:") && said.contains("Moved: Party:"),
        "{said}"
    );
    assert!(
        said.contains("Skipped: Taken:") && said.contains("already exists"),
        "{said}"
    );
    assert!(said.contains("Updated: Unbuilt:"), "{said}");
    assert!(
        said.contains("Left ") && said.contains("Removed the empty folder"),
        "{said}"
    );
    // the folders: moved with their saves; the other engine's build and its folder untouched
    assert_eq!(read(&format!("{a}/nxrs/Kart/Kart_data/save/slot")), "save");
    assert_eq!(
        read(&format!("{a}/SwitchRecomp Builds/Kart/Kart.exe")),
        "old engine"
    );
    assert!(!Path::new(&format!("{a}/SwitchRecomp Builds/Kart [nxrs]")).exists());
    assert!(Path::new(&format!("{b}/nxrs/Party/Party.exe")).is_file());
    assert!(Path::new(&format!("{b}/SwitchRecomp Builds/Taken [nxrs]/Taken.exe")).is_file());
    assert!(!Path::new(&format!("{c}/SwitchRecomp Builds")).exists());
    assert!(Path::new(&format!("{c}/nxrs/Solo/Solo.exe")).is_file());
    // the library: buildDir follows, Taken stays; the file before is kept twice (Qt rule + this move)
    let v = library(&lib);
    let dirs: Vec<&str> = (0..4)
        .map(|i| v["games"][i]["buildDir"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(dirs[0], format!("{a}/nxrs/Kart"));
    assert_eq!(dirs[1], format!("{b}/nxrs/Party"));
    assert_eq!(dirs[2], format!("{b}/SwitchRecomp Builds/Taken [nxrs]"));
    assert_eq!(dirs[3], format!("{b}/nxrs/Unbuilt"));
    assert_eq!(read(&format!("{dir}/library.pre-layout.json")), before);
    assert_eq!(read(&format!("{dir}/library.qt-backup.json")), before);
    assert!(
        read(&lib).contains("\n    \"games\": ["),
        "the Qt format: 4-space indent"
    );
    // again: Taken is still blocked, nothing else to do
    let again = text(&nxrs(
        &format!("{dir}/data"),
        &["--library", &lib, "--migrate-builds"],
    ));
    assert!(
        !again.contains("Moved:") && again.contains("Skipped: Taken:"),
        "{again}"
    );
    // --select-title moves that one only
    std::fs::remove_dir_all(format!("{b}/nxrs/Taken")).unwrap();
    let one = nxrs(
        &format!("{dir}/data"),
        &[
            "--library",
            &lib,
            "--migrate-builds",
            "--select-title",
            "Taken",
        ],
    );
    assert_eq!(one.status.code(), Some(0), "{}", text(&one));
    assert!(Path::new(&format!("{b}/nxrs/Taken/Taken_data/save/slot")).is_file());
}

/// A game whose files are open (running) is skipped: its folder is not moved, its entry not changed.
#[cfg(windows)]
#[test]
fn a_folder_in_use_stays() {
    use std::os::windows::fs::OpenOptionsExt;
    let (_t, dir) = tmp();
    build(&dir, "Kart");
    let lib = format!("{dir}/library.json");
    let games = [entry("1", "Kart", &dir)];
    write(
        &lib,
        json!({"schema": 1, "games": games}).to_string().as_bytes(),
    );
    let exe = format!("{dir}/SwitchRecomp Builds/Kart [nxrs]/Kart.exe");
    // a running game's image: shared for reading only
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&exe)
        .unwrap();
    let out = nxrs(
        &format!("{dir}/data"),
        &["--library", &lib, "--migrate-builds"],
    );
    drop(held);
    let said = text(&out);
    assert!(
        said.contains("Skipped: Kart:") && said.contains("in use"),
        "{said}"
    );
    assert!(Path::new(&exe).is_file());
    assert!(read(&lib).contains("Kart [nxrs]"));
}

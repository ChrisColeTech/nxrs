//! Where builds go (owner, 2026-11-26): `<source folder>\nxrs\<name>`, the old layout's folders moved
//! there, other engines' folders never reused; a build with its own data never "Source offline".

use super::build_layout::{build_dir_for, migrated_build_dir, nxrs_folder_for};
use super::card_state::{card_state, status_for};
use super::facts::Facts;

// The tests below use Windows drive paths ("P:/…"), which only Windows treats as absolute.
#[cfg(not(windows))]
#[test]
fn new_builds_go_to_the_nxrs_folder_beside_the_source_unix() {
    assert_eq!(
        build_dir_for("/dumps/Kart [v0]/Kart.nsp", "", "Kart"),
        "/dumps/Kart [v0]/nxrs/Kart"
    );
    assert_eq!(
        migrated_build_dir("/dumps/Kart/SwitchRecomp Builds/Kart [nxrs]").as_deref(),
        Some("/dumps/Kart/nxrs/Kart")
    );
    assert_eq!(
        nxrs_folder_for("/d/SwitchRecomp Builds/Kart"),
        "/d/nxrs/Kart"
    );
}

#[cfg(windows)]
#[test]
fn new_builds_go_to_the_nxrs_folder_beside_the_source() {
    assert_eq!(
        build_dir_for("P:/Dumps/Kart [v0]/Kart.nsp", "", "Kart"),
        "P:/Dumps/Kart [v0]/nxrs/Kart"
    );
    // the Build location setting is the base instead of the source's folder
    assert_eq!(
        build_dir_for("P:/D/Kart.nsp", "E:\\Builds\\", "Kart"),
        "E:/Builds/nxrs/Kart"
    );
}

#[cfg(windows)]
#[test]
fn only_the_old_default_layout_migrates() {
    let old = "P:/Dumps/Arceus/SwitchRecomp Builds/Pokemon Legends - Arceus 1G1U [nxrs]";
    assert_eq!(
        migrated_build_dir(old).as_deref(),
        Some("P:/Dumps/Arceus/nxrs/Pokemon Legends - Arceus 1G1U")
    );
    assert_eq!(
        migrated_build_dir("P:\\D\\switchrecomp builds\\Kart [nxrs]\\").as_deref(),
        Some("P:/D/nxrs/Kart")
    );
    // another engine's build beside it, a custom folder, the new layout: left alone
    for keep in [
        "P:/D/SwitchRecomp Builds/Kart",
        "E:/Mine/Kart [nxrs]",
        "P:/D/nxrs/Kart",
        "P:/D/SwitchRecomp Builds/ [nxrs]",
    ] {
        assert_eq!(migrated_build_dir(keep), None, "{keep}");
    }
}

#[cfg(windows)]
#[test]
fn other_engine_entries_get_a_folder_of_their_own() {
    assert_eq!(
        nxrs_folder_for("P:/D/SwitchRecomp Builds/Kart"),
        "P:/D/nxrs/Kart"
    );
    assert_eq!(
        nxrs_folder_for("P:/D/SwitchRecomp Builds/Kart [nxrs]"),
        "P:/D/nxrs/Kart"
    );
    assert_eq!(nxrs_folder_for("E:/Mine/Kart"), "E:/Mine/nxrs/Kart");
    assert_eq!(nxrs_folder_for("E:/Mine/nxrs/Kart/"), "E:/Mine/nxrs/Kart");
}

#[test]
fn a_build_with_its_own_data_is_never_offline() {
    // in_place: the build still reads its package (a build from before); gone: offline
    let old = Facts {
        has_build: true,
        in_place: true,
        source_available: false,
        ..Facts::default()
    };
    assert_eq!(status_for(&old, false, false, false), "Source offline");
    let carried = Facts {
        in_place: false,
        ..old
    };
    assert_eq!(status_for(&carried, false, false, false), "Ready to play");
    let s = card_state(&carried, false, false, false, true);
    assert!(s.action == "Play" && s.action_enabled && !s.needs_attention);
}

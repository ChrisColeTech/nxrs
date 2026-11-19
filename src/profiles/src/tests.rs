use super::*;
use patches::{Layout, PatchFile};

#[test]
fn every_profile_parses() {
    let (n, errors) = check();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(n, 187, "nxnative e4b0d9b has 187 profiles (dm/games holds 192 entries: the 187 folders and 5 files)");
    assert_eq!(all().len(), n);
    let with_patches = all().iter().filter(|p| p.patches.is_some()).count();
    assert_eq!(with_patches, 27, "27 patches.py");
}

/// The owner's seven games, with the main build ids of the owner's own packages (read with `nxrs profile
/// --package`): the profile each resolves to, and the build row when the build is registered.
#[test]
fn owner_games() {
    let cases: [(u64, &str, &str, Option<&str>, u64); 7] = [
        // Mario Kart 8 Deluxe, the combined 1G+1U+1D NSP (update 4.0.1)
        (0x0100152000022000, "158c0c7399aabd7bf83795c2ef3484c7", "0100152000022000", Some("v1507328"), 19_200_000),
        // Mario Kart World, the NUP: its main (with subsdk1) is the 1.8.0 program; the profile registers v0 only
        (0x0400C3F00006E000, "d00e4f399e411d0613aa8d26b1c02fef", "0400C3F00006E000", None, 31_250_000),
        // Donkey Kong Bananza, NUP v458752
        (0x040074A01BF12000, "e25c90c1c136638885659fd55bf8a03b3403489d", "040074A01BF12000", Some("v458752"), 31_250_000),
        // Ocarina of Time, NUP v131072 (the package reports the update's title id, ...800)
        (0x0400486024DEC800, "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348", "0400486024DEC000", Some("v131072"), 31_250_000),
        // Pokémon Legends: Z-A, the Switch 2 Edition NUP: the same main as the 2.0.2 update (a Switch 1 program)
        (0x0100F43008C44000, "b1f12fd919eae86ab8a978317677e64bce443d1f", "0100F43008C44000", Some("v393216"), 19_200_000),
        // Pokémon Legends: Arceus, 1G+1U NSP (1.1.1)
        (0x01001F5010DFA000, "aee8f150dda1b5a838806e1a5ea6827ad9f3c51e", "01001F5010DFA000", Some("v262144"), 19_200_000),
        // Pokémon Violet, 1G+1U+1D NSP v786432; the profile registers 1.0.0 only
        (0x01008F6008C5E000, "709bfd66115298640155fcc4979dba151c7cc79a", "01008F6008C5E000", None, 19_200_000),
    ];
    for (tid, bid, id, version, hz) in cases {
        let m = lookup(Some(tid), Some(bid)).unwrap_or_else(|| panic!("{tid:016X}: no profile"));
        assert_eq!(m.profile.id, id, "{tid:016X}");
        assert_eq!(m.build.map(|b| b.version.as_str()), version, "{tid:016X}");
        assert_eq!(m.how, if version.is_some() { How::Build } else { How::Title }, "{tid:016X}");
        assert_eq!(m.tick_hz(), hz, "{tid:016X}");
    }
    // by build id alone, as a release under another title id would be found
    let (p, b) = by_build("158c0c7399aabd7bf83795c2ef3484c7").unwrap();
    assert_eq!((p.id.as_str(), b.display.as_str()), ("0100152000022000", "4.0.1"));
    assert_eq!(get("0100152000022000").unwrap().memory.app_pool, 0xCD50_0000);
    assert!(lookup(Some(0x0100000000999000), None).is_none());
}

/// nxnative → TOML (the exporter) → Rust → TOML → Rust: every table survives unchanged.
#[test]
fn patch_tables_round_trip() {
    let mut files: Vec<&PatchFile> = all().iter().flat_map(|p| p.patch_files()).collect();
    files.extend(shared_patches());
    let mut entries = 0;
    for f in &files {
        let text = f.to_toml().unwrap();
        let back = PatchFile::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", f.source));
        assert_eq!(&back, *f, "{}", f.source);
        entries += f.cut.len() + f.obs.len() + f.stub.len() + f.matches.len() + f.capability.len() + f.binding.len();
    }
    assert_eq!(files.len(), 27 + 1 + 2, "27 patches.py, the owner's turbo.toml, sdk + engine");
    assert!(entries > 150, "{entries}");
}

/// Every recorded stub, placed at the layout the exporter recorded it at, gives back its own words: the
/// relocations and the words agree (the exporter checked them against nxnative at a third layout).
#[test]
fn recorded_stubs_place_back() {
    let mut bodies = vec![];
    for f in all().iter().flat_map(|p| p.patch_files()).chain(shared_patches()) {
        bodies.extend(f.stub.iter().map(|s| (&f.source, &s.body, vec![])));
        bodies.extend(f.matches.iter().filter_map(|m| m.stub.as_ref().map(|b| (&f.source, b, m.others.iter().map(|o| o.label.clone()).collect::<Vec<_>>()))));
    }
    let mut placed = 0;
    for (src, b, labels) in &bodies {
        if !b.evaluated {
            assert!(b.place(&Layout { area: 0, state: 0, base: 0, others: &[] }).is_err());
            continue;
        }
        let mut labels = labels.clone();
        labels.sort();
        let others: Vec<(&str, u64)> = labels.iter().enumerate().map(|(i, l)| (l.as_str(), 0x0200_0000 + i as u64 * 0x0010_0000)).collect();
        let at = Layout { area: 0x0700_0000, state: 0x0600_0000, base: 0x0100_0000, others: &others };
        let p = b.place(&at).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(p.code, b.code, "{src}");
        for (put, (_, words)) in b.puts.iter().zip(&p.puts) {
            assert_eq!(&put.words, words, "{src}");
        }
        placed += 1;
    }
    assert!(placed >= 10, "{placed} stubs placed");
}

#[test]
fn za_60fps_is_applied_at_build() {
    let mods = [("main", "b1f12fd919eae86ab8a978317677e64bce443d1f00000000"), ("sdk", "df7a3960c74c3a4803f826c5871532523aa9cf2a")];
    let (text, said) = translator_patches(&mods).unwrap();
    assert!(text.contains("set za_60fps NXRS_PATCH_ZA_60FPS 1\n"), "{text}");
    assert!(text.contains("site za_60fps main 0x1b296d8 39c08134 2a1f03f4"), "{text}");
    assert_eq!(said.len(), 1);
    // another build of main gets nothing
    let (text, _) = translator_patches(&[("main", "b1f12fd919eae86ab8a978317677e64bce443d1e")]).unwrap();
    assert!(text.is_empty());
}

#[test]
fn settings_and_player_rows() {
    let mkw = get("0400C3F00006E000").unwrap();
    assert_eq!((mkw.settings.fps_target, mkw.settings.pad_layout.as_str()), (60, "full"));
    assert_eq!(mkw.platform(), Platform::Switch2);
    let rows = player_settings();
    assert!(rows.iter().any(|r| r.variable == "DM_FPS_TARGET" && r.default == "DM_GAME_FPS_TARGET"));
    assert!(source().starts_with("nxnative "));
}

#[test]
fn shader_tables_are_embedded() {
    let t = shader_tables("0100152000022000").unwrap();
    let names: Vec<&str> = t.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["pso_manifest.tsv", "pso_pairs.tsv", "shader_pairs.tsv", "shader_facts.tsv"]);
    assert_eq!(t[2].1.lines().count(), 19013);
    let seed = prewarm_shaders("0100152000022000").unwrap();
    assert!(seed.len() > 500 && seed.iter().all(|(k, s)| k.len() >= 16 && !s.is_empty()), "{}", seed.len());
    assert!(shader_tables("0000000000000000").unwrap().is_empty());
}

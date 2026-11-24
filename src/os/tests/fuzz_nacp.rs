//! nxnative's NACP fuzz corpus (`dm/tools/fuzz/corpus/nacp`) through the boot's reader of a title's control data
//! (`boot::nacp_version`): read or refused, never a panic.

#[test]
fn nacp_corpus_is_read_or_refused_cleanly() {
    let d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/fuzz/nacp");
    let mut n = 0;
    for e in std::fs::read_dir(&d).unwrap().flatten() {
        let data = std::fs::read(e.path()).unwrap();
        let r = std::panic::catch_unwind(|| nxos::boot::nacp_version(&data));
        assert!(
            r.is_ok(),
            "{}: the NACP reader panicked",
            e.path().display()
        );
        n += 1;
    }
    assert!(n > 0, "no corpus");
}

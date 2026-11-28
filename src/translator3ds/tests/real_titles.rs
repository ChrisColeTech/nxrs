//! opt-in checks over a real decrypted title the wave owns nothing of: name it with
//! `NXRS_3DS_TITLE=<path to .3ds/.cia/.cxi>` and this reads it, translates it, and reports counts.
//! Never runs anything; skipped without the env, like the translator's real package tests.

use translator3ds::Mods;

#[test]
fn a_real_title_reads_and_translates() {
    let Some(path) = std::env::var_os("NXRS_3DS_TITLE") else {
        eprintln!("skipped: set NXRS_3DS_TITLE to a decrypted .3ds/.cia/.cxi to run this");
        return;
    };
    let opened = translator3ds::title::open(std::path::Path::new(&path))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let programs = translator3ds::programs(&opened.title, Mods::default()).unwrap();
    for (name, program) in &programs {
        let analysis = translator3ds::discover::analyze(program);
        println!("{name}: {} bytes of text, {} functions, {} svc sites", program.text.bytes.len(), analysis.functions.len(), analysis.svc_sites);
        assert!(!analysis.functions.is_empty(), "{name}: no functions found");
    }
    assert!(programs.iter().any(|(name, _)| name == "executable"));
}

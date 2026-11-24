//! Differential check of `translator::xlate::simd` against mkw-recomp's simd_c.py on real instruction words.
//! Generate the dumps first (scripts/simd/dump_py.py, see its header), then:
//!   cargo test -p translator --test simd_words -- --ignored --nocapture
//! Optional: SIMD_DUMP_DIR (default <repo>/build/simd). Every *.tsv in it is checked.

use std::path::PathBuf;

#[test]
#[ignore]
fn simd_words_match_python() {
    let dir = std::env::var("SIMD_DUMP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/simd"));
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("dump dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "tsv"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no *.tsv in {}", dir.display());
    let mut total_bad = 0;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let (mut ok, mut bad, mut both_unsup) = (0, 0, 0);
        for line in text.lines() {
            let mut it = line.splitn(4, '\t');
            let (kind, wh, pch, exp) = (
                it.next().unwrap(),
                it.next().unwrap(),
                it.next().unwrap(),
                it.next().unwrap(),
            );
            let w = u32::from_str_radix(wh, 16).unwrap();
            let pc = u64::from_str_radix(pch, 16).unwrap();
            let r = match kind {
                "dp" => translator::xlate::simd::dp(w, pc),
                "ldst" => translator::xlate::simd::ldst(w, pc),
                _ => translator::xlate::simd::crc(w),
            };
            let got = match r {
                Ok(s) => s,
                Err(e) => format!("UNSUP:{}", e.0),
            };
            if got == exp {
                ok += 1;
                if exp.starts_with("UNSUP:") {
                    both_unsup += 1;
                }
            } else {
                bad += 1;
                if bad <= 20 {
                    eprintln!("MISMATCH {kind} {wh}\n  py: {exp}\n  rs: {got}");
                }
            }
        }
        println!(
            "{}: {} matched ({} both-unsupported), {} mismatched",
            f.display(),
            ok,
            both_unsup,
            bad
        );
        total_bad += bad;
    }
    assert_eq!(total_bad, 0);
}

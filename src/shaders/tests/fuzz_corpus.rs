//! nxnative's fuzz corpus for its shader splitter and Unreal pak reader (`dm/tools/fuzz/corpus/shader_split`,
//! `uepak`): every input goes through this crate's readers of the same formats (`formats::scan_file`, the scan's entry
//! for one file's bytes; `formats::unreal::pak_files` / `pak_member`) and must be read or refused cleanly: problems
//! listed, never a panic.

use std::path::PathBuf;

fn corpus(h: &str) -> Vec<(String, Vec<u8>)> {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/fuzz")
        .join(h);
    let mut v: Vec<_> = std::fs::read_dir(&d)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();
    v.sort();
    assert!(!v.is_empty(), "{h}: no corpus");
    v
}

#[test]
fn shader_split_corpus_is_scanned_cleanly() {
    for (name, data) in corpus("shader_split") {
        let r = std::panic::catch_unwind(|| {
            let mut out = shaders::formats::Scanned::default();
            shaders::formats::scan_file(
                &shaders::formats::Ctx::default(),
                &data,
                &format!("/{name}"),
                &mut out,
            );
            (out.found.len(), out.problems.len())
        });
        let (f, p) = r.unwrap_or_else(|_| panic!("{name}: a reader panicked"));
        eprintln!("{name}: {f} shaders, {p} problems");
    }
}

#[test]
fn uepak_corpus_is_read_or_refused_cleanly() {
    for (name, data) in corpus("uepak") {
        let r = std::panic::catch_unwind(|| {
            let read = |o: u64, n: u64| -> Result<Vec<u8>, String> {
                let (o, n) = (o as usize, n as usize);
                data.get(o..o.checked_add(n).ok_or("overflow")?)
                    .map(|s| s.to_vec())
                    .ok_or_else(|| "past the end".to_string())
            };
            let files = shaders::formats::unreal::pak_files(&read, data.len() as u64, None);
            let mut out = shaders::formats::Scanned::default();
            shaders::formats::unreal::pak_member(&data, &name, &format!("/{name}"), &mut out);
            (files.map(|f| f.len()), out.problems.len())
        });
        match r {
            Ok((files, p)) => eprintln!("{name}: {files:?} entries, {p} problems"),
            Err(_) => panic!("{name}: the pak reader panicked"),
        }
    }
}

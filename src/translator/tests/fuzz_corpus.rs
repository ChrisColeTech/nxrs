//! nxnative's fuzz corpus (`dm/tools/fuzz/corpus/`: inputs its fuzz harnesses found that broke a reader once) through
//! this translator's readers of the same formats: every input is read or refused cleanly (an error, never a panic,
//! never an allocation the file cannot account for). NSO modules (`loader::Nso::parse`) and RomFS images
//! (`package::romfs::RomFs::from_image`).

use std::path::PathBuf;
use std::sync::Arc;

fn corpus(h: &str) -> Vec<(String, Vec<u8>)> {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/fuzz").join(h);
    let mut v: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
    v.sort();
    assert!(!v.is_empty(), "{h}: no corpus");
    v
}

#[test]
fn nso_corpus_is_read_or_refused_cleanly() {
    for (name, data) in corpus("nso") {
        match std::panic::catch_unwind(|| translator::loader::Nso::parse(&data).map(|n| (n.size, n.image.len()))) {
            Ok(Ok((size, len))) => assert_eq!(size, len, "{name}: the image has the length its segments lay out"),
            Ok(Err(e)) => eprintln!("{name}: refused ({e:?})"),
            Err(_) => panic!("{name}: the NSO reader panicked"),
        }
    }
}

#[test]
fn romfs_corpus_is_read_or_refused_cleanly() {
    for (name, data) in corpus("romfs") {
        let r = std::panic::catch_unwind(|| {
            let st: Arc<dyn translator::package::storage::Storage> = Arc::new(data.clone());
            translator::package::romfs::RomFs::from_image(st, name.clone()).map(|_| ())
        });
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("{name}: refused ({e})"),
            Err(_) => panic!("{name}: the RomFS reader panicked"),
        }
    }
}

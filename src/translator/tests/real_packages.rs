//! Checks against the owner's real dumps (multi-GB, on P:). Ignored by default:
//!   cargo test -p translator --release -- --ignored
//! Skips (passes) any input that is not present on this machine. Keys come from default_keys_path().
use std::path::{Path, PathBuf};
use translator::package::{open_exefs, SourceKind};

const DUMPS: &str = r"P:\Games\Nintendo\DUMPS";

fn find_nsp(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .map_or(false, |x| x.eq_ignore_ascii_case("nsp"))
        })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

#[test]
#[ignore]
fn mario_kart_world_nup() {
    let p = Path::new(DUMPS)
        .join("Mario Kart World [0400C3F00006E000][v0]")
        .join("Mario Kart World [0400C3F00006E000][v0].nup");
    if !p.exists() {
        return eprintln!("skip: {}", p.display());
    }
    let e = open_exefs(&p, None).unwrap();
    assert_eq!(e.kind, SourceKind::Nup);
    assert_eq!(e.title_id, Some(0x0400C3F00006E000));
    for f in ["main", "main.npdm", "rtld", "sdk", "subsdk0", "subsdk1"] {
        assert!(e.files.contains_key(f), "{f}");
    }
    assert_eq!(
        hex(&e.main_build_id().unwrap()),
        "D00E4F399E411D0613AA8D26B1C02FEF"
    );
}

#[test]
#[ignore]
fn arceus_combined_nsp_takes_the_update() {
    let combined =
        find_nsp(&Path::new(DUMPS).join("Pokemon Legends Arceus [01001F5010DFA800][v262144]"));
    let base = PathBuf::from(
        r"P:\Games\Nintendo\DUMPS\Pokemon Legends Arceus base [01001F5010DFA000][v0]\Pokemon Legends Arceus base [01001F5010DFA000][v0].nsp",
    );
    let (Some(combined), true) = (combined, base.exists()) else {
        return eprintln!("skip: Arceus packages not present");
    };
    if translator::package::default_keys_path().is_none() {
        return eprintln!("skip: no prod.keys");
    }
    let u = open_exefs(&combined, None).unwrap();
    let b = open_exefs(&base, None).unwrap();
    assert_eq!((u.kind, b.kind), (SourceKind::Nsp, SourceKind::Nsp));
    assert_eq!(u.title_id, Some(0x01001F5010DFA000));
    assert_eq!((u.version, b.version), (Some(262144), Some(0)));
    assert_eq!(
        hex(&u.main_build_id().unwrap()),
        "AEE8F150DDA1B5A838806E1A5EA6827AD9F3C51E"
    );
    assert_eq!(
        hex(&b.main_build_id().unwrap()),
        "7FCAD279539DE183B25C11834FD4A030591CFE25"
    );
}

#[test]
#[ignore]
fn mario_kart_8_deluxe_folder_matches_nsp() {
    let mk8 = Path::new(DUMPS).join("Mario Kart 8 Deluxe");
    let folder =
        mk8.join(r"SwitchRecomp Builds\Mario Kart 8 Deluxe 1G1U1D\Mario Kart 8 Deluxe 1G1U1D_data");
    if !folder.exists() {
        return eprintln!("skip: {}", folder.display());
    }
    let f = open_exefs(&folder, None).unwrap();
    assert_eq!(f.kind, SourceKind::Folder);
    assert_eq!(
        hex(&f.main_build_id().unwrap()),
        "158C0C7399AABD7BF83795C2EF3484C7"
    );
    if let (Some(nsp), Some(_)) = (find_nsp(&mk8), translator::package::default_keys_path()) {
        let n = open_exefs(&nsp, None).unwrap();
        assert_eq!(n.files, f.files);
        assert_eq!(n.version, Some(1507328));
    }
}

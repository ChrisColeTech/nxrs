//! Play finds a web build the CLI made under another folder name (WEB-W11: Beach Bounce's web build
//! sat in `nxrs-web\Beach Bounce Remastered`, the library's output name ends in "nsw2u"), by the
//! title id in its `build.json`.
mod common;

use common::*;
use nxrs_launcher::model::web_build::web_dir;
use nxrs_launcher::services::build_facts::facts;
use nxrs_launcher::services::web_serve::played_web_dir;

/// A `main.npdm` whose ACI0 names `id`.
fn npdm(id: u64) -> Vec<u8> {
    let mut b = vec![0u8; 0x100];
    b[..4].copy_from_slice(b"META");
    b[0x70..0x74].copy_from_slice(&0x80u32.to_le_bytes());
    b[0x80..0x84].copy_from_slice(b"ACI0");
    b[0x90..0x98].copy_from_slice(&id.to_le_bytes());
    b
}

fn web_build(root: &str, title_id: &str) {
    let json = format!(
        r#"{{"format":1,"kind":"nxrs-web","title":"X","titleId":"{title_id}","entry":"index.html","data":"X_data"}}"#
    );
    write(&format!("{root}/build.json"), json.as_bytes());
    write(&format!("{root}/game.wasm"), b"\0asm");
}

#[test]
fn play_finds_a_web_build_named_otherwise_by_title_id() {
    let (_t, dir) = tmp();
    let mut g = built_game(&dir);
    g.output_name = "Kart nsw2u".into();
    g.build_dir = format!("{dir}/nxrs/Kart nsw2u");
    write(
        &format!("{}/exefs/main.npdm", g.data_dir()),
        &npdm(0x0100_aaaa_0000_c000),
    );
    let own = web_dir(&g);
    // nothing built for the web: the own folder, and no web build
    assert_eq!(played_web_dir(&g), own);
    assert!(!facts(&g).web_build);
    // another title's build beside it is not taken
    web_build(&format!("{dir}/nxrs-web/Other"), "0100bbbb0000c000");
    assert_eq!(played_web_dir(&g), own);
    // the CLI's build under the package's name, same title id: found
    let cli = format!("{dir}/nxrs-web/Kart");
    web_build(&cli, "0100AAAA0000C000");
    assert_eq!(played_web_dir(&g), cli);
    assert!(facts(&g).web_build);
    // once the own folder holds a build, it wins
    web_build(&own, "0100aaaa0000c000");
    assert_eq!(played_web_dir(&g), own);
}

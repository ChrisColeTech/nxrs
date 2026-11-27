//! Import build takes a web build (LAUNCH-ART-WEBIMPORT): a web game's folder (its `index.html`
//! found a few folders down, as the GTA V port keeps it) becomes an entry that plays in the browser,
//! served at the root of its own `<slug>.localhost`; an nxrs web build joins the entry of its game.
mod common;

use common::*;
use nxrs_launcher::flows::surface::Surface;
use nxrs_launcher::model::web_build::imported_page;
use nxrs_launcher::services::browser::Browser;
use nxrs_launcher::services::web_import::{Found, find};
use nxrs_launcher::services::web_serve::{WebServer, open_site};
use std::io::{Read, Write};
use std::net::TcpStream;

/// A web game laid out as `P:\Games\Web\GTA5` is: the site two folders down, root-absolute paths.
fn web_game(dir: &str) -> (String, String) {
    let top = format!("{dir}/Web/GTA5");
    let site = format!("{top}/mirror/playgta5.com");
    write(
        &format!("{site}/index.html"),
        b"<script src=\"/b/x.js\"></script>",
    );
    write(&format!("{site}/b/x.js"), b"go()");
    write(&format!("{site}/data/f.bin"), b"0123456789");
    write(&format!("{top}/launcher-settings.json"), b"{}");
    (top, format!("{site}/index.html"))
}

/// One GET to the server at `port` for `host`, answered in full (the server closes after it).
fn get(port: u16, host: &str, path: &str, extra: &str) -> (u16, String, Vec<u8>) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req =
        format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n{extra}\r\n");
    c.write_all(req.as_bytes()).unwrap();
    let mut all = Vec::new();
    c.read_to_end(&mut all).unwrap();
    let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&all[..split]).into_owned();
    (
        head[9..12].parse().unwrap(),
        head,
        all[split + 4..].to_vec(),
    )
}

#[test]
fn a_web_game_folder_imports_and_plays_from_its_own_host() {
    let (_t, dir) = tmp();
    let (top, page) = web_game(&dir);
    assert_eq!(find(&top), Ok(Found::Page(page.clone())));
    let mut app = app_with(&dir, vec![]);
    app.tools.browser = Some(Browser {
        name: "Chrome".into(),
        program: FAKE.into(),
    });
    assert!(app.import_build(&top));
    let g = app.lib.games[0].clone();
    assert_eq!(
        (g.title.as_str(), imported_page(&g)),
        ("GTA5", Some(page.as_str()))
    );
    assert_eq!(g.build_dir, "");
    // Play has one place to go: the browser; Build has none (no source)
    assert!(app.why_not(&g.id, false, true).is_none());
    assert!(app.why_not(&g.id, false, false).is_some());
    assert!(app.why_not(&g.id, true, true).is_some());
    // the card plays it and nothing needs attention; the status says where it plays
    let card = app.card_of(&g);
    assert_eq!(
        (card.action.as_str(), card.needs_attention),
        ("Play", false)
    );
    assert_eq!(app.status_of(&g), "Ready to play in the browser");
    assert_eq!(g.format(), "WEB BUILD");
    // the same folder again selects the entry
    app.selected.clear();
    assert!(app.import_build(&page));
    assert_eq!(app.lib.games.len(), 1);
    assert_eq!(app.selected, g.id);
    // the key is written for this entry only
    let saved = library(&format!("{dir}/library.json"));
    assert_eq!(saved["games"][0]["webBuild"], page.as_str());

    let server = WebServer::start(0).unwrap();
    let url = server
        .mount_site(&g.output_name, open_site(&page).unwrap())
        .unwrap();
    let port = server.port();
    assert_eq!(url, format!("http://gta5.localhost:{port}/"));
    let (status, head, body) = get(port, "gta5.localhost", "/", "");
    assert_eq!(
        (status, body.as_slice()),
        (200, &b"<script src=\"/b/x.js\"></script>"[..])
    );
    assert!(
        head.contains("Cross-Origin-Embedder-Policy: require-corp"),
        "{head}"
    );
    let (status, _, body) = get(port, "gta5.localhost", "/b/x.js?v=1", "");
    assert_eq!((status, body.as_slice()), (200, &b"go()"[..]));
    let (status, head, body) = get(
        port,
        "gta5.localhost",
        "/data/f.bin",
        "Range: bytes=2-4\r\n",
    );
    assert_eq!((status, body.as_slice()), (206, &b"234"[..]));
    assert!(head.contains("Content-Range: bytes 2-4/10"), "{head}");
    // nothing outside the site, and the site is not served on another host
    assert_eq!(
        get(port, "gta5.localhost", "/../launcher-settings.json", "").0,
        404
    );
    assert_eq!(get(port, "127.0.0.1", "/b/x.js", "").0, 404);
}

#[test]
fn an_nxrs_web_build_joins_its_games_entry_and_play_asks() {
    let (_t, dir) = tmp();
    let mut g = built_game(&dir);
    let mut npdm = vec![0u8; 0x100];
    npdm[..4].copy_from_slice(b"META");
    npdm[0x70..0x74].copy_from_slice(&0x80u32.to_le_bytes());
    npdm[0x80..0x84].copy_from_slice(b"ACI0");
    npdm[0x90..0x98].copy_from_slice(&0x0100_aaaa_0000_c000u64.to_le_bytes());
    write(&format!("{}/exefs/main.npdm", g.data_dir()), &npdm);
    g.id = "kart".into();
    let web = format!("{dir}/elsewhere/Kart web");
    let json =
        r#"{"format":1,"kind":"nxrs-web","title":"Kart","titleId":"0100AAAA0000C000","data":"d"}"#;
    write(&format!("{web}/build.json"), json.as_bytes());
    write(&format!("{web}/game.wasm"), b"\0asm");
    write(&format!("{web}/index.html"), b"page");
    let mut app = app_with(&dir, vec![g]);
    app.tools.browser = Some(Browser {
        name: "Chrome".into(),
        program: FAKE.into(),
    });
    // picked as its page: still the nxrs build, joined to Kart's entry
    assert!(app.import_build(&format!("{web}/index.html")));
    assert_eq!(app.lib.games.len(), 1);
    assert_eq!(app.lib.games[0].web_build, web);
    // both versions play: Play asks Native or Web
    app.ask_target(false);
    assert!(matches!(app.surfaces.last(), Some(Surface::Choices { .. })));
}

#[test]
fn a_folder_without_a_web_build_is_refused_and_a_dropped_one_imports() {
    let (_t, dir) = tmp();
    write(&format!("{dir}/empty/readme.txt"), b"x");
    let mut app = app_with(&dir, vec![]);
    assert!(!app.import_build(&format!("{dir}/empty")));
    assert!(app.lib.games.is_empty());
    let (top, _) = web_game(&dir);
    app.drop_files(&[top]);
    assert_eq!(app.lib.games.len(), 1);
    // two pages at one depth: the player is asked to pick the page itself
    write(&format!("{dir}/two/a/index.html"), b"a");
    write(&format!("{dir}/two/b/index.html"), b"b");
    assert!(find(&format!("{dir}/two")).is_err());
}

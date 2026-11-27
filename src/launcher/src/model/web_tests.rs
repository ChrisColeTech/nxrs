//! The web build's rules: its folder, `build.json`, slugs, routes, ranges, the contract's arguments.

use super::args;
use super::game::Game;
use super::http_range::{Answer, answer, content_type, request_line};
use super::web_build::{Target, WebBuild, chromium_browser, parse, percent_decode, route, slug};
use super::web_build::{url, web_dir};

/// An absolute path as the library holds one: under a drive on Windows, under `/` elsewhere.
fn abs(rest: &str) -> String {
    if cfg!(windows) {
        format!("P:/{rest}")
    } else {
        format!("/{rest}")
    }
}

fn game() -> Game {
    Game {
        source: abs("Games/Kart/Kart.nsp"),
        build_dir: abs("Games/Kart/nxrs/Kart DX"),
        output_name: "Kart DX".into(),
        ..Game::default()
    }
}

#[test]
fn web_builds_go_beside_the_native_one() {
    assert_eq!(web_dir(&game()), abs("Games/Kart/nxrs-web/Kart DX"));
    let none = Game::default();
    assert_eq!(web_dir(&none), "");
}

#[test]
fn build_for_web_is_the_build_with_target_web_into_the_web_folder() {
    let g = game();
    let a = args::build_web(&g, "C:/keys/prod.keys", true, 8);
    let native = args::build(&g, "C:/keys/prod.keys", true, 8);
    let dir = |a: &[String]| a[a.iter().position(|x| x == "--outdir").unwrap() + 1].clone();
    assert_eq!(
        dir(&native),
        super::path_text::native(&abs("Games/Kart/nxrs/Kart DX"))
    );
    assert_eq!(
        dir(&a),
        super::path_text::native(&abs("Games/Kart/nxrs-web/Kart DX"))
    );
    assert_eq!(&a[a.len() - 2..], ["--target", "web"]);
    // the native build's data folder: the shaders its runs translated
    assert_eq!(
        &a[a.len() - 4..a.len() - 2],
        [
            "--shaders-from".to_string(),
            super::path_text::native(&abs("Games/Kart/nxrs/Kart DX/Kart DX_data"))
        ]
    );
    // everything else is the native build's: source, name, jobs, --machine, update, keys
    assert_eq!(a.len(), native.len() + 4);
    assert_eq!(a[0], "build");
    for x in ["-o", "Kart DX", "--jobs", "8", "--machine", "--keys"] {
        assert!(a.iter().any(|y| y == x), "{x}");
    }
}

#[test]
fn build_json_is_read_and_checked() {
    let ok = r#"{"format":1,"kind":"nxrs-web","title":"Kart DX","entry":"index.html",
        "data":"Kart DX_data","files":[],"shaders":[]}"#;
    let b = parse(ok).unwrap();
    let want = WebBuild {
        title: "Kart DX".into(),
        data: "Kart DX_data".into(),
        entry: "index.html".into(),
    };
    assert_eq!(b, want);
    assert!(parse("not json").is_err());
    assert!(parse(r#"{"format":1,"kind":"other","data":"x"}"#).is_err());
    assert!(
        parse(r#"{"format":2,"kind":"nxrs-web","data":"x"}"#)
            .unwrap_err()
            .contains("2")
    );
    // a data folder outside the build is refused; a strange entry falls back to index.html
    for data in ["", "..", "../x", "a/b", "C:x"] {
        let t = format!(r#"{{"format":1,"kind":"nxrs-web","data":"{data}"}}"#);
        assert!(parse(&t).is_err(), "{data}");
    }
    let t = r#"{"format":1,"kind":"nxrs-web","data":"d","entry":"../x.html"}"#;
    assert_eq!(parse(t).unwrap().entry, "index.html");
}

#[test]
fn slugs_and_addresses() {
    assert_eq!(slug("Beach Bounce Remastered"), "beach-bounce-remastered");
    assert_eq!(slug("  Kart: DX (v1.0)!"), "kart-dx-v1-0");
    assert_eq!(slug("★"), "game");
    assert_eq!(url(8780, "kart"), "http://127.0.0.1:8780/kart/");
}

#[test]
fn routes_stay_inside_the_build() {
    let b = |p: &str| Some(Target::Build(p.into()));
    assert_eq!(route("", "index.html"), b("index.html"));
    assert_eq!(route("game.wasm", "index.html"), b("game.wasm"));
    assert_eq!(
        route("data/romfs/a%20b.bin", "i"),
        Some(Target::Data("romfs/a b.bin".into()))
    );
    assert_eq!(route("data", "i"), b("data"));
    for bad in [
        "../x",
        "data/../../x",
        "a//b",
        "%2e%2e/x",
        "a\\b",
        "C:/x",
        "a/./b",
        "%zz",
        "x%00",
    ] {
        assert_eq!(route(bad, "i"), None, "{bad}");
    }
    assert_eq!(percent_decode("caf%C3%A9"), Some("café".into()));
    assert_eq!(percent_decode("%ff"), None);
}

#[test]
fn ranges_answer_exactly() {
    assert_eq!(answer(None, 100), Answer::Whole);
    assert_eq!(answer(Some("bytes=0-9"), 100), Answer::Part(0, 9));
    assert_eq!(answer(Some("bytes=90-"), 100), Answer::Part(90, 99));
    assert_eq!(answer(Some("bytes=95-200"), 100), Answer::Part(95, 99));
    assert_eq!(answer(Some("bytes=-10"), 100), Answer::Part(90, 99));
    assert_eq!(answer(Some("bytes=-500"), 100), Answer::Part(0, 99));
    assert_eq!(answer(Some("bytes=100-"), 100), Answer::Unsatisfiable);
    assert_eq!(answer(Some("bytes=-0"), 100), Answer::Unsatisfiable);
    assert_eq!(answer(Some("bytes=0-0"), 0), Answer::Unsatisfiable);
    // what HTTP lets a server ignore: the whole file
    for whole in ["bytes=9-1", "bytes=0-1,5-6", "items=0-1", "bytes=x-y"] {
        assert_eq!(answer(Some(whole), 100), Answer::Whole, "{whole}");
    }
}

#[test]
fn types_and_request_lines() {
    assert_eq!(content_type("a/game.wasm"), "application/wasm");
    assert_eq!(
        content_type("io-worker.js"),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(content_type("build.json"), "application/json");
    assert_eq!(content_type("romfs/x.bin"), "application/octet-stream");
    let r = request_line("GET /kart/game.wasm?v=1 HTTP/1.1");
    assert_eq!(r, Some(("GET".into(), "/kart/game.wasm".into())));
    assert_eq!(request_line("GET kart HTTP/1.1"), None);
    assert_eq!(request_line("GET /kart SPDY/3"), None);
}

#[test]
fn chromium_browsers_by_prog_id() {
    assert_eq!(chromium_browser("ChromeHTML"), Some("Chrome"));
    assert_eq!(chromium_browser("msedgehtm"), Some("Edge"));
    assert_eq!(chromium_browser("FirefoxURL-308046B0AF4A39CB"), None);
}

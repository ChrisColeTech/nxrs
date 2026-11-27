//! Build and Play answered Web: the server's answers (headers, ranges, the data folder, paths that
//! leave the build), `--serve-web`, and the flow from Build's chooser through the fake engine's web
//! build to the served page.
mod common;

use common::*;
use nxrs_launcher::constants::{words_target as t, words_web as w};
use nxrs_launcher::flows::surface::Surface;
use nxrs_launcher::flows::{Action, Outcome};
use nxrs_launcher::model::web_build::web_dir;
use nxrs_launcher::services::browser::Browser;
use nxrs_launcher::services::web_serve::{WebServer, open_build};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

/// A web folder as nxrs-build writes it, with a 10-byte data file.
fn web_folder(dir: &str) -> String {
    let root = format!("{dir}/nxrs-web/Kart");
    write(
        &format!("{root}/build.json"),
        br#"{"format":1,"kind":"nxrs-web","title":"Kart","entry":"index.html","data":"Kart_data"}"#,
    );
    write(&format!("{root}/index.html"), b"<!doctype html>page");
    write(&format!("{root}/game.wasm"), b"\0asm1234");
    write(&format!("{root}/Kart_data/romfs/a b.bin"), b"0123456789");
    // the GPU's shader files: not in data.json, fetched by URL on first use
    write(
        &format!("{root}/Kart_data/shaders/spv/0aF3.spv"),
        b"\x03\x02\x23\x07spv",
    );
    write(&format!("{dir}/nxrs-web/secret.txt"), b"outside");
    root
}

struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

/// One request on an open connection (keep-alive), its answer read by Content-Length.
fn ask(c: &mut BufReader<TcpStream>, line: &str, extra: &str) -> Reply {
    let req = format!("{line} HTTP/1.1\r\nHost: x\r\n{extra}\r\n");
    c.get_mut().write_all(req.as_bytes()).unwrap();
    let mut head = String::new();
    loop {
        let mut l = String::new();
        c.read_line(&mut l).unwrap();
        head.push_str(&l);
        if l == "\r\n" {
            break;
        }
    }
    let n: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .unwrap()
        .parse()
        .unwrap();
    let mut body = vec![0; if line.starts_with("HEAD") { 0 } else { n }];
    c.read_exact(&mut body).unwrap();
    let status = head[9..12].parse().unwrap();
    Reply { status, head, body }
}

fn connect(url: &str) -> (BufReader<TcpStream>, String) {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    (
        BufReader::new(TcpStream::connect(host).unwrap()),
        path.to_string(),
    )
}

#[test]
fn the_server_answers_with_isolation_and_exact_ranges() {
    let (_t, dir) = tmp();
    let root = web_folder(&dir);
    let (build, mount) = open_build(&root).unwrap();
    assert_eq!(build.data, "Kart_data");
    let server = WebServer::start(0).unwrap();
    let url = server.mount("Kart", mount).unwrap();
    assert_eq!(url, format!("http://127.0.0.1:{}/kart/", server.port()));
    let (mut c, base) = connect(&url);
    let page = ask(&mut c, &format!("GET {base}"), "");
    assert_eq!(
        (page.status, page.body.as_slice()),
        (200, &b"<!doctype html>page"[..])
    );
    for h in [
        "Cross-Origin-Opener-Policy: same-origin",
        "Cross-Origin-Embedder-Policy: require-corp",
        "Accept-Ranges: bytes",
        "Content-Type: text/html",
    ] {
        assert!(page.head.contains(h), "{h} in {}", page.head);
    }
    // the same connection again: keep-alive; the module's type
    let wasm = ask(&mut c, &format!("GET {base}game.wasm"), "");
    assert!(wasm.head.contains("Content-Type: application/wasm"));
    let data = format!("GET {base}data/romfs/a%20b.bin");
    let part = ask(&mut c, &data, "Range: bytes=2-5\r\n");
    assert_eq!((part.status, part.body.as_slice()), (206, &b"2345"[..]));
    assert!(part.head.contains("Content-Range: bytes 2-5/10"));
    assert!(part.head.contains("Content-Length: 4"));
    let tail = ask(&mut c, &data, "Range: bytes=-3\r\n");
    assert_eq!((tail.status, tail.body.as_slice()), (206, &b"789"[..]));
    let past = ask(&mut c, &data, "Range: bytes=10-\r\n");
    assert_eq!(past.status, 416);
    assert!(past.head.contains("Content-Range: bytes */10"));
    let head = ask(&mut c, &format!("HEAD {base}game.wasm"), "");
    assert!(head.status == 200 && head.head.contains("Content-Length: 8"));
    // nothing outside the build, no unknown build, no other method
    for bad in [
        "../secret.txt",
        "%2e%2e/secret.txt",
        "data/../../secret.txt",
        "nope.js",
    ] {
        assert_eq!(
            ask(&mut c, &format!("GET {base}{bad}"), "").status,
            404,
            "{bad}"
        );
    }
    assert_eq!(ask(&mut c, "GET /other/index.html", "").status, 404);
    assert_eq!(ask(&mut c, &format!("PUT {base}"), "").status, 405);
    // the page's log posts are taken (204) and their bodies read past: the connection goes on
    // (`ask` ends the head with CRLF, which here ends the 7-byte body "hello\r\n" instead)
    let log = ask(
        &mut c,
        &format!("POST {base}log"),
        "Content-Length: 7\r\n\r\nhello",
    );
    assert_eq!(log.status, 204);
    let spv = ask(&mut c, &format!("GET {base}data/shaders/spv/0aF3.spv"), "");
    assert_eq!((spv.status, &spv.body[4..]), (200, &b"spv"[..]));
    let bare = ask(&mut c, "GET /kart", "");
    assert!(bare.status == 301 && bare.head.contains("Location: /kart/"));
}

#[test]
fn serve_web_serves_a_folder_from_the_command_line() {
    let (_t, dir) = tmp();
    let root = web_folder(&dir);
    let mut child = std::process::Command::new(NXRS)
        .args(["--serve-web", &root])
        .env("NXRS_WEB_PORT", "0")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line
        .split(" at ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_string();
    let (mut c, base) = connect(&url);
    let got = ask(
        &mut c,
        &format!("GET {base}data/romfs/a%20b.bin"),
        "Range: bytes=0-0\r\n",
    );
    child.kill().unwrap();
    let _ = child.wait();
    assert_eq!(
        (got.status, got.body.as_slice()),
        (206, &b"0"[..]),
        "{line}"
    );
}

#[test]
fn build_web_then_play_web() {
    let (_t, dir) = tmp();
    let mut g = built_game(&dir);
    g.build_dir = format!("{dir}/nxrs/Kart");
    std::fs::create_dir_all(g.data_dir()).unwrap();
    write(&g.executable(), b"fixture");
    write(&g.code_module(), b"fixture");
    let mut app = app_with(&dir, vec![g.clone()]);
    // built natively, no web build yet: Build asks, and only Web can build
    assert_eq!(app.why_not("g", true, false), Some(t::UP_TO_DATE.into()));
    assert_eq!(app.why_not("g", false, true), Some(t::NO_WEB_BUILD.into()));
    app.dispatch(Action::Build);
    let chooser = Surface::Target {
        game_id: "g".into(),
        build: true,
    };
    assert_eq!(app.surfaces, vec![chooser]);
    app.surfaces.clear();
    // the chooser's Web answer
    app.dispatch(Action::BuildWeb);
    app.wait_idle(std::time::Duration::from_secs(20));
    assert_eq!(app.outcomes, vec![Outcome::Smoke(true)]);
    assert_eq!(web_dir(&g), format!("{dir}/nxrs-web/Kart"));
    // the fake engine wrote a web folder only because it was asked for `--target web`
    assert!(read(&format!("{}/Kart.build.log", web_dir(&g))).contains(w::DONE));
    // the native build is untouched; the web one is there, but no browser: Web cannot play
    assert!(app.facts_of("g").playable() && app.facts_of("g").web_build);
    assert_eq!(app.why_not("g", false, true), Some(t::NO_BROWSER.into()));
    let browser = Browser {
        name: "Test".into(),
        program: FAKE.into(),
    };
    app.tools.browser = Some(browser);
    assert_eq!(app.why_not("g", false, true), None);
    app.dispatch(Action::PlayWeb);
    let url = app.job_label.split(": ").nth(1).unwrap().to_string();
    assert!(url.ends_with("/kart/"), "{}", app.job_label);
    let (mut c, base) = connect(&url);
    let got = ask(
        &mut c,
        &format!("GET {base}data/romfs/a.bin"),
        "Range: bytes=8-\r\n",
    );
    assert_eq!((got.status, got.body.as_slice()), (206, &b"89"[..]));
}

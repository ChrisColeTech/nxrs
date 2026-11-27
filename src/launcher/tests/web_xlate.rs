//! The web server's shader translation route (WEB-W7b): what it refuses (sizes, keys, bodies,
//! unknown builds, no translator), what it writes (only the key's folder under the build's
//! `shaders\live`), the fake engine's translations through it, and (opt-in, `NXRS_XLATE_EXE`) a
//! recorded Beach Bounce shader translated by the real nxrs-build.
mod common;

use common::*;
use nxrs_launcher::constants::words_web as w;
use nxrs_launcher::services::web_serve::{WebServer, open_build};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

const KEY: &str = "0123456789abcdef";

/// A web folder (build.json, page, module, an empty shader library) served with `translator`.
fn serve(dir: &str, translator: &str) -> (WebServer, String, String) {
    let root = format!("{dir}/nxrs-web/Kart");
    write(
        &format!("{root}/build.json"),
        br#"{"format":1,"kind":"nxrs-web","title":"Kart","entry":"index.html","data":"Kart_data"}"#,
    );
    write(&format!("{root}/index.html"), b"page");
    write(&format!("{root}/game.wasm"), b"\0asm");
    std::fs::create_dir_all(format!("{root}/Kart_data/shaders/spv")).unwrap();
    let (_, mut mount) = open_build(&root).unwrap();
    mount.translator = translator.into();
    let server = WebServer::start(0).unwrap();
    let url = server.mount("Kart", mount).unwrap();
    (server, url, format!("{root}/Kart_data/shaders"))
}

fn body(ctl: &[u8], code: &[u8]) -> Vec<u8> {
    let mut v = (ctl.len() as u32).to_le_bytes().to_vec();
    v.extend_from_slice(ctl);
    v.extend_from_slice(code);
    v
}

/// One POST on its own connection: (status, head, body); `length` overrides Content-Length.
fn post(url: &str, path: &str, data: &[u8], length: Option<usize>) -> (u16, String, Vec<u8>) {
    let host = url.trim_start_matches("http://").split('/').next().unwrap();
    let mut c = BufReader::new(TcpStream::connect(host).unwrap());
    let n = length.unwrap_or(data.len());
    let head = format!("POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {n}\r\n\r\n");
    c.get_mut().write_all(head.as_bytes()).unwrap();
    if length.is_none() {
        c.get_mut().write_all(data).unwrap();
    }
    let mut head = String::new();
    loop {
        let mut l = String::new();
        c.read_line(&mut l).unwrap();
        head.push_str(&l);
        if l == "\r\n" || l.is_empty() {
            break;
        }
    }
    let n: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .unwrap()
        .parse()
        .unwrap();
    let mut got = vec![0; n];
    c.read_exact(&mut got).unwrap();
    (head[9..12].parse().unwrap(), head, got)
}

/// Every file under `dir`, relative.
fn files(dir: &str) -> Vec<String> {
    let mut out = vec![];
    let mut todo = vec![std::path::PathBuf::from(dir)];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.push(
                    p.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_route_refuses_bad_requests_and_writes_nothing() {
    let (_t, dir) = tmp();
    let (_s, url, shaders) = serve(&dir, "");
    let before = files(&dir);
    let ok = body(b"ctl", b"code");
    let route = |k: &str| format!("/kart/shader-xlate/{k}");
    // keys a file may not be named by; an unknown build; bodies that are not a shader
    for k in [
        "../../x",
        "0123",
        "zzzzzzzzzzzzzzzz",
        "0123456789abcdef0",
        "%2e%2e%2fsecret",
    ] {
        assert_eq!(post(&url, &route(k), &ok, None).0, 400, "{k}");
    }
    assert_eq!(
        post(&url, &format!("{}/more", route(KEY)), &ok, None).0,
        400
    );
    assert_eq!(
        post(&url, &format!("/nope/shader-xlate/{KEY}"), &ok, None).0,
        404
    );
    assert_eq!(post(&url, &route(KEY), &[9, 0, 0, 0, 1, 2], None).0, 400);
    assert_eq!(post(&url, &route(KEY), &body(b"", b"code"), None).0, 400);
    assert_eq!(post(&url, &route(KEY), &body(b"ctl", b""), None).0, 400);
    // a body past the limit: refused unread, the connection closed
    let (status, head, _) = post(&url, &route(KEY), &[], Some(5 << 20));
    assert!(
        status == 413 && head.contains("Connection: close"),
        "{head}"
    );
    // no translator: said, not faked
    let (status, head, text) = post(&url, &route(KEY), &ok, None);
    assert_eq!(
        (status, String::from_utf8_lossy(&text).as_ref()),
        (503, w::NO_TRANSLATOR)
    );
    assert!(head.contains("Cross-Origin-Embedder-Policy: require-corp"));
    assert_eq!(files(&dir), before, "nothing written");
    // the page's other posts are still taken as before
    assert_eq!(post(&url, "/kart/log", b"hello", None).0, 204);
    // a translation already in the live folder is answered from it, translator or not
    write(
        &format!("{shaders}/live/{KEY}/spv/{KEY}.spv"),
        b"\x03\x02\x23\x07",
    );
    write(
        &format!("{shaders}/live/{KEY}/spv/{KEY}.json"),
        b"{\"j\":1}",
    );
    let (status, _, got) = post(&url, &route(&KEY.to_uppercase()), &ok, None);
    assert_eq!(
        (status, got),
        (200, body(b"\x03\x02\x23\x07", b"{\"j\":1}"))
    );
}

#[test]
fn the_engine_translates_into_the_keys_live_folder_only() {
    let (_t, dir) = tmp();
    let (_s, url, shaders) = serve(&dir, FAKE);
    let before = files(&dir);
    let route = format!("/kart/shader-xlate/{KEY}");
    // the fake engine "translates" a code that is the key, writing the ctl as the SPIR-V
    let (status, _, got) = post(&url, &route, &body(b"SPIRV", KEY.as_bytes()), None);
    assert_eq!((status, got), (200, body(b"SPIRV", b"{}")));
    let mut now = files(&dir);
    now.retain(|f| !before.contains(f));
    let live = format!("nxrs-web/Kart/Kart_data/shaders/live/{KEY}/spv");
    assert_eq!(
        now,
        vec![format!("{live}/{KEY}.json"), format!("{live}/{KEY}.spv")]
    );
    assert!(std::path::Path::new(&format!("{shaders}/live/{KEY}/spv/{KEY}.spv")).is_file());
    // a translation of another key than asked for, and a refusal: 422 with the engine's reason
    let other = "fedcba9876543210";
    let (status, _, text) = post(
        &url,
        &format!("/kart/shader-xlate/{other}"),
        &body(b"S", KEY.as_bytes()),
        None,
    );
    let text = String::from_utf8_lossy(&text).to_string();
    assert!(
        status == 422 && text.contains(other) && text.contains(KEY),
        "{text}"
    );
    let (status, _, text) = post(
        &url,
        &format!("/kart/shader-xlate/{other}"),
        &body(b"S", b"junk"),
        None,
    );
    assert!(status == 422 && String::from_utf8_lossy(&text).contains("refused junk"));
}

/// Opt-in: `NXRS_XLATE_EXE=<nxrs-build>` and `NXRS_XLATE_INPUT=<recorded request>.bin` (a page's
/// POST body, named `<key>.bin`): the real engine translates it through the server.
#[test]
fn a_recorded_shader_is_translated_by_the_real_engine() {
    let (Ok(exe), Ok(input)) = (
        std::env::var("NXRS_XLATE_EXE"),
        std::env::var("NXRS_XLATE_INPUT"),
    ) else {
        return;
    };
    let key = std::path::Path::new(&input)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let (_t, dir) = tmp();
    let (_s, url, shaders) = serve(&dir, &exe);
    let request = std::fs::read(&input).unwrap();
    let t0 = std::time::Instant::now();
    let (status, _, got) = post(&url, &format!("/kart/shader-xlate/{key}"), &request, None);
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&got));
    eprintln!("translated {key} in {:.2} s", t0.elapsed().as_secs_f64());
    let n = u32::from_le_bytes(got[..4].try_into().unwrap()) as usize;
    assert_eq!(&got[4..8], &[0x03, 0x02, 0x23, 0x07], "SPIR-V magic");
    let spv = std::fs::read(format!("{shaders}/live/{key}/spv/{key}.spv")).unwrap();
    assert_eq!(
        &got[4..4 + n],
        spv.as_slice(),
        "the answer is the library's file"
    );
    let json = read(&format!("{shaders}/live/{key}/spv/{key}.json"));
    assert!(!json.is_empty() && got[4 + n..] == *json.as_bytes());
    assert!(read(&format!("{shaders}/live/{key}/manifest.tsv")).contains(&key));
}

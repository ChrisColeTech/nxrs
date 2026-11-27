//! The web server's pad bridge (WEB-PADBRIDGE): a WebSocket on `/<slug>/pad-bridge` is upgraded
//! with RFC 6455's accept key, each pad line the page sends reaches `nxrs-build pad-bridge`'s
//! stdin (the fake engine writes it into `pad-bridge.log`), other text does not, ping is answered,
//! and when the page goes the bridge's stdin closes and it ends. Refusals: no key, no build, no
//! nxrs-build.
mod common;

use common::*;
use nxrs_launcher::constants::words_web as w;
use nxrs_launcher::services::web_serve::{WebServer, open_build};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// A web folder served with `bridge` as its nxrs-build: (server, host:port, the log's path).
fn serve(dir: &str, bridge: &str) -> (WebServer, String, String) {
    let root = format!("{dir}/nxrs-web/Kart");
    write(
        &format!("{root}/build.json"),
        br#"{"format":1,"kind":"nxrs-web","title":"Kart","entry":"index.html","data":"Kart_data"}"#,
    );
    write(&format!("{root}/index.html"), b"page");
    write(&format!("{root}/game.wasm"), b"\0asm");
    std::fs::create_dir_all(format!("{root}/Kart_data")).unwrap();
    let (_, mut mount) = open_build(&root).unwrap();
    mount.translator = bridge.into();
    let server = WebServer::start(0).unwrap();
    server.mount("Kart", mount).unwrap();
    let host = format!("127.0.0.1:{}", server.port());
    (server, host, format!("{root}/pad-bridge.log"))
}

/// Sends the upgrade request; the answer's head.
fn upgrade(host: &str, path: &str, key: Option<&str>) -> (BufReader<TcpStream>, String) {
    let mut c = BufReader::new(TcpStream::connect(host).unwrap());
    let key = key
        .map(|k| format!("Sec-WebSocket-Key: {k}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         {key}Sec-WebSocket-Version: 13\r\n\r\n"
    );
    c.get_mut().write_all(head.as_bytes()).unwrap();
    let mut got = String::new();
    loop {
        let mut l = String::new();
        if c.read_line(&mut l).unwrap() == 0 || l == "\r\n" {
            break;
        }
        got.push_str(&l);
    }
    (c, got)
}

/// A client frame (masked, as a browser sends it).
fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [1u8, 2, 3, 4];
    let mut f = vec![0x80 | opcode, 0x80 | payload.len() as u8];
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

fn wait_for(path: &str, text: &str) -> String {
    let t0 = Instant::now();
    loop {
        let log = std::fs::read_to_string(path).unwrap_or_default();
        if log.contains(text) || t0.elapsed() > Duration::from_secs(10) {
            return log;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn pad_lines_reach_the_bridge_until_the_page_goes() {
    let (_t, dir) = tmp();
    let (_s, host, log) = serve(&dir, FAKE);
    let (mut c, head) = upgrade(&host, "/kart/pad-bridge", Some("dGhlIHNhbXBsZSBub25jZQ=="));
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(
        head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "{head}"
    );
    let s = c.get_mut();
    s.write_all(&frame(1, b"pad 1 32768 0 0 0 0 0 0 0"))
        .unwrap();
    s.write_all(&frame(1, b"rumble please")).unwrap();
    s.write_all(&frame(9, b"hi")).unwrap();
    let mut pong = [0u8; 4];
    c.read_exact(&mut pong).unwrap();
    assert_eq!(pong, [0x8a, 2, b'h', b'i'], "ping answered");
    c.get_mut()
        .write_all(&frame(1, b"pad 0 0 0 0 0 0 0 0 0"))
        .unwrap();
    let got = wait_for(&log, "pad 0 0 0");
    assert!(
        got.contains("fake pad bridge: pad 1 32768 0 0 0 0 0 0 0"),
        "{got}"
    );
    assert!(
        !got.contains("rumble please"),
        "only pad lines are forwarded: {got}"
    );
    // the page goes: a close frame, then the bridge's stdin closes and it ends
    c.get_mut().write_all(&frame(8, &[])).unwrap();
    let got = wait_for(&log, "fake pad bridge: end");
    assert!(
        got.ends_with("fake pad bridge: end\n") || got.ends_with("end\r\n"),
        "{got}"
    );
}

#[test]
fn a_dropped_socket_ends_the_bridge_too() {
    let (_t, dir) = tmp();
    let (_s, host, log) = serve(&dir, FAKE);
    let (mut c, head) = upgrade(&host, "/kart/pad-bridge", Some("x3JJHMbDL1EzLkh9GBhXDw=="));
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    c.get_mut()
        .write_all(&frame(1, b"pad 1 0 0 0 0 0 0 0 0"))
        .unwrap();
    wait_for(&log, "pad 1 0 0");
    drop(c);
    assert!(wait_for(&log, "fake pad bridge: end").contains("end"));
}

#[test]
fn refusals() {
    let (_t, dir) = tmp();
    let (_s, host, _) = serve(&dir, "");
    let (_, head) = upgrade(&host, "/kart/pad-bridge", None);
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    let (_, head) = upgrade(&host, "/nope/pad-bridge", Some("a2V5"));
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    let (mut c, head) = upgrade(&host, "/kart/pad-bridge", Some("a2V5"));
    assert!(head.starts_with("HTTP/1.1 503"), "no nxrs-build: {head}");
    let mut text = String::new();
    let _ = c.read_to_string(&mut text);
    assert_eq!(text, w::NO_PAD_BRIDGE);
}

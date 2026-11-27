//! The web server's encodings and caching (docs/35 W6): a whole-file GET that takes gzip gets the
//! build's `<file>.gz` with `Content-Encoding`, a range always the identity bytes (so the page's
//! exact-range checks hold), and files are revalidated by ETag (304).
mod common;

use common::*;
use nxrs_launcher::model::http_cache::{accepts_gzip, etag, not_modified, use_gzip};
use nxrs_launcher::services::web_serve::{WebServer, open_build};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

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
    let mut body = vec![0; n];
    c.read_exact(&mut body).unwrap();
    let status = head[9..12].parse().unwrap();
    Reply { status, head, body }
}

fn header<'a>(r: &'a Reply, name: &str) -> Option<&'a str> {
    r.head
        .lines()
        .find_map(|l| l.strip_prefix(name)?.strip_prefix(": "))
}

#[test]
fn cache_rules() {
    assert!(accepts_gzip(Some("gzip, deflate, br, zstd")));
    assert!(accepts_gzip(Some("br;q=1.0, GZIP;q=0.5")));
    assert!(!accepts_gzip(Some("gzip;q=0, br")));
    assert!(!accepts_gzip(Some("br, zstd")) && !accepts_gzip(None));
    // a range, an older copy or no copy: the identity bytes
    assert!(use_gzip(None, Some("gzip"), 5, Some(5)));
    assert!(!use_gzip(Some("bytes=0-9"), Some("gzip"), 5, Some(9)));
    assert!(!use_gzip(None, Some("gzip"), 9, Some(5)));
    assert!(!use_gzip(None, Some("gzip"), 5, None));
    let t = etag(16, 0xabc, true);
    assert_eq!(t, "\"10-abc-gz\"");
    assert_ne!(t, etag(16, 0xabc, false));
    assert!(not_modified(Some(&format!("W/\"x\", {t}")), None, &t));
    assert!(!not_modified(Some(&t), Some("bytes=0-1"), &t));
    assert!(!not_modified(None, None, &t));
}

#[test]
fn gzip_copies_whole_files_identity_ranges_and_revalidation() {
    let (_t, dir) = tmp();
    let root = format!("{dir}/nxrs-web/Kart");
    write(
        &format!("{root}/build.json"),
        br#"{"format":1,"kind":"nxrs-web","title":"Kart","entry":"index.html","data":"Kart_data"}"#,
    );
    write(&format!("{root}/index.html"), b"page");
    write(&format!("{root}/game.wasm"), b"\0asm12345678");
    // nxrs-build writes the copy after the module: at least as new
    write(&format!("{root}/game.wasm.gz"), b"GZIPPED");
    write(&format!("{root}/zdata/romfs/a"), b"0123456789");
    write(&format!("{root}/Kart_data/romfs/a"), b"raw");
    let (_, mount) = open_build(&root).unwrap();
    let server = WebServer::start(0).unwrap();
    let url = server.mount("Kart", mount).unwrap();
    let rest = url.strip_prefix("http://").unwrap();
    let (host, base) = rest.split_at(rest.find('/').unwrap());
    let mut c = BufReader::new(TcpStream::connect(host).unwrap());
    let gz = "Accept-Encoding: gzip, deflate, br\r\n";
    let whole = ask(&mut c, &format!("GET {base}game.wasm"), gz);
    assert_eq!(
        (whole.status, whole.body.as_slice()),
        (200, &b"GZIPPED"[..])
    );
    assert_eq!(header(&whole, "Content-Encoding"), Some("gzip"));
    assert_eq!(header(&whole, "Content-Type"), Some("application/wasm"));
    assert_eq!(header(&whole, "Cache-Control"), Some("no-cache"));
    assert!(
        whole
            .head
            .contains("Cross-Origin-Embedder-Policy: require-corp")
    );
    // no gzip accepted: the module itself, under another tag
    let plain = ask(&mut c, &format!("GET {base}game.wasm"), "");
    assert_eq!(plain.body, b"\0asm12345678");
    assert_eq!(header(&plain, "Content-Encoding"), None);
    let (tz, tp) = (
        header(&whole, "ETag").unwrap(),
        header(&plain, "ETag").unwrap(),
    );
    assert_ne!(tz, tp);
    // a range of the module: identity bytes, exact 206, even when gzip is accepted
    let part = ask(
        &mut c,
        &format!("GET {base}game.wasm"),
        &format!("{gz}Range: bytes=4-7\r\n"),
    );
    assert_eq!((part.status, part.body.as_slice()), (206, &b"1234"[..]));
    assert_eq!(header(&part, "Content-Range"), Some("bytes 4-7/12"));
    assert_eq!(header(&part, "Content-Encoding"), None);
    // revalidation: the tag of what would be sent answers 304 with no body
    let again = format!("{gz}If-None-Match: {tz}\r\n");
    let nm = ask(&mut c, &format!("GET {base}game.wasm"), &again);
    assert_eq!((nm.status, nm.body.len()), (304, 0));
    assert_eq!(header(&nm, "ETag"), Some(tz));
    // the identity tag does not match the gzip answer
    let other = ask(
        &mut c,
        &format!("GET {base}game.wasm"),
        &format!("{gz}If-None-Match: {tp}\r\n"),
    );
    assert_eq!(other.status, 200);
    // compressed data extents are files of the build, read by exact range
    let z = ask(
        &mut c,
        &format!("GET {base}zdata/romfs/a"),
        "Range: bytes=2-3\r\n",
    );
    assert_eq!((z.status, z.body.as_slice()), (206, &b"23"[..]));
    // an answer that is not a file is never kept
    let missing = ask(&mut c, &format!("GET {base}nope"), "");
    assert_eq!(header(&missing, "Cache-Control"), Some("no-store"));
}

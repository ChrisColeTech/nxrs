//! The pad bridge's WebSocket pieces (WEB-PADBRIDGE): SHA-1 against FIPS 180's vectors, the
//! accept key against RFC 6455's example, frame heads, masking, server frames, the route and the
//! pad line rule (the engine's `input::bridge::parse`).

use super::ws::*;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn sha1_vectors() {
    assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    assert_eq!(
        hex(&sha1(b"abc")),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    let two_blocks = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    assert_eq!(
        hex(&sha1(two_blocks)),
        "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
    );
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        hex(&sha1(&million)),
        "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
    );
}

#[test]
fn accept_key_of_rfc6455() {
    assert_eq!(
        accept("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

#[test]
fn frame_heads_and_masking() {
    let h = head([0x81, 0x80 | 27]);
    assert_eq!((h.fin, h.opcode, h.masked, h.len7), (true, TEXT, true, 27));
    let h = head([0x88, 0xfe]);
    assert_eq!((h.opcode, h.len7), (CLOSE, 126), "a 16-bit length follows");
    assert!(!head([0x01, 0x85]).fin, "a fragment");
    let key = [0x37, 0xfa, 0x21, 0x3d];
    // RFC 6455 5.7: "Hello" masked
    let mut data = [0x7f, 0x9f, 0x4d, 0x51, 0x58];
    unmask(&mut data, key);
    assert_eq!(&data, b"Hello");
}

#[test]
fn server_frames() {
    assert_eq!(frame(PONG, b"Hi"), vec![0x8a, 2, b'H', b'i']);
    assert_eq!(frame(CLOSE, &[]), vec![0x88, 0]);
    assert_eq!(frame(PONG, &[0; 300]).len(), 127, "at most 125 bytes");
}

#[test]
fn the_pad_route() {
    assert_eq!(
        pad_route("/beach-bounce/pad-bridge").as_deref(),
        Some("beach-bounce")
    );
    assert_eq!(
        pad_route("/beach-bounce/pad-bridge?x=1").as_deref(),
        Some("beach-bounce")
    );
    assert_eq!(pad_route("/beach-bounce/pad-bridge/x"), None);
    assert_eq!(pad_route("//pad-bridge"), None);
    assert_eq!(pad_route("/beach-bounce/index.html"), None);
}

#[test]
fn pad_lines_as_the_engine_reads_them() {
    assert!(pad_line_ok("pad 1 32768 0 0 0 0 0 0 65535"));
    assert!(pad_line_ok("pad 0 0 0 0 0 0 0 0 0"));
    assert!(!pad_line_ok("pad 2 0 0 0 0 0 0 0 0"));
    assert!(!pad_line_ok("pad 1 0 0 0 0 0 0 0"));
    assert!(!pad_line_ok("pad 1 0 0 0 0 0 0 0 0 0"));
    assert!(!pad_line_ok("pad 1 65536 0 0 0 0 0 0 0"));
    assert!(!pad_line_ok("rumble 1 0 0 0 0 0 0 0 0"));
    assert!(!pad_line_ok(""));
}

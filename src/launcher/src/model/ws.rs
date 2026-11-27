//! The little of RFC 6455 the pad bridge needs (WEB-PADBRIDGE): the handshake's accept key
//! (SHA-1, FIPS 180-4, then base64), a frame head read from its first two bytes, the client's
//! masking undone, and the short unmasked frames a server sends (pong, close).

use crate::constants::web as c;
use base64::Engine;

pub const TEXT: u8 = 1;
pub const CLOSE: u8 = 8;
pub const PING: u8 = 9;
pub const PONG: u8 = 10;

/// SHA-1 of `data`.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut cc, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & cc) | (!b & d), 0x5A82_7999),
                1 => (b ^ cc ^ d, 0x6ED9_EBA1),
                2 => ((b & cc) | (b & d) | (cc & d), 0x8F1B_BCDC),
                _ => (b ^ cc ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            (e, d, cc, b, a) = (d, cc, b.rotate_left(30), a, t);
        }
        for (s, v) in h.iter_mut().zip([a, b, cc, d, e]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 20];
    for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(h) {
        *o = s.to_be_bytes();
    }
    out
}

/// `Sec-WebSocket-Accept` for the client's `Sec-WebSocket-Key`.
pub fn accept(key: &str) -> String {
    let digest = sha1(format!("{}{}", key.trim(), c::WS_GUID).as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// A frame's first two bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Head {
    pub fin: bool,
    pub opcode: u8,
    pub masked: bool,
    /// 0..125 the length; 126 a 16-bit length follows, 127 a 64-bit one
    pub len7: u8,
}

pub fn head(b: [u8; 2]) -> Head {
    Head {
        fin: b[0] & 0x80 != 0,
        opcode: b[0] & 0x0f,
        masked: b[1] & 0x80 != 0,
        len7: b[1] & 0x7f,
    }
}

/// The client's payload unmasked in place.
pub fn unmask(data: &mut [u8], key: [u8; 4]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= key[i % 4];
    }
}

/// A server frame (unmasked, final) of at most 125 bytes, as pong and close are.
pub fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let p = &payload[..payload.len().min(125)];
    let mut out = vec![0x80 | (opcode & 0x0f), p.len() as u8];
    out.extend_from_slice(p);
    out
}

/// `/<slug>/pad-bridge`: the slug.
pub fn pad_route(path: &str) -> Option<String> {
    let path = path.split('?').next().unwrap_or_default();
    let mut parts = path.trim_start_matches('/').split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(slug), Some(seg), None) if seg == c::PAD_SEGMENT && !slug.is_empty() => {
            Some(slug.to_string())
        }
        _ => None,
    }
}

/// A line the bridge takes: `pad <0|1>` then eight levels of 0..65535 (os `input::bridge::parse`).
pub fn pad_line_ok(line: &str) -> bool {
    let mut it = line.split_ascii_whitespace();
    if it.next() != Some(c::PAD_WORD) || !matches!(it.next(), Some("0" | "1")) {
        return false;
    }
    let levels: Vec<&str> = it.collect();
    levels.len() == c::PAD_LEVELS && levels.iter().all(|v| v.parse::<u16>().is_ok())
}

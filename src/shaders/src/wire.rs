//! The live translator's request and answer as bytes, for a web build whose page asks its server to translate a
//! shader the game made at run time (docs/waves/WEB-W7b.md; `POST <page>/shader-xlate/<key>`, and the
//! `nxrs-build shader-xlate` that the server runs reads the same request on stdin).
//!
//!   request: u32 LE ctl length | ctl | code         (the two blobs the native path hands `prebuild::translate_one`)
//!   answer:  u32 LE spv length | spv | json         (the translation's `spv/<key>.spv` and its reflection json)

/// The largest request taken (ctl + code); the server refuses a longer body unread.
pub const MAX_REQUEST: usize = 4 << 20;

pub fn request(ctl: &[u8], code: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + ctl.len() + code.len());
    v.extend_from_slice(&(ctl.len() as u32).to_le_bytes());
    v.extend_from_slice(ctl);
    v.extend_from_slice(code);
    v
}

/// (ctl, code); None when the body is not a request (shorter than its ctl, too long, or no code).
pub fn parse_request(b: &[u8]) -> Option<(&[u8], &[u8])> {
    if b.len() > MAX_REQUEST {
        return None;
    }
    let n = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
    let ctl = b.get(4..4usize.checked_add(n)?)?;
    let code = &b[4 + n..];
    (!ctl.is_empty() && !code.is_empty()).then_some((ctl, code))
}

pub fn answer(spv: &[u8], json: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + spv.len() + json.len());
    v.extend_from_slice(&(spv.len() as u32).to_le_bytes());
    v.extend_from_slice(spv);
    v.extend_from_slice(json);
    v
}

/// (SPIR-V words, reflection json); None when the answer is not one (no whole words, no module).
pub fn parse_answer(b: &[u8]) -> Option<(Vec<u32>, Option<String>)> {
    let n = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
    let spv = b.get(4..4usize.checked_add(n)?)?;
    if spv.is_empty() || spv.len() % 4 != 0 {
        return None;
    }
    let words = spv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let json = String::from_utf8(b[4 + n..].to_vec())
        .ok()
        .filter(|s| !s.is_empty());
    Some((words, json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_an_answer_survive_the_round_trip() {
        let r = request(b"ctl!", b"code bytes");
        assert_eq!(parse_request(&r), Some((&b"ctl!"[..], &b"code bytes"[..])));
        let a = answer(&[1, 0, 0, 0, 2, 0, 0, 0], b"{\"x\":1}");
        assert_eq!(parse_answer(&a), Some((vec![1, 2], Some("{\"x\":1}".into()))));
        assert_eq!(parse_answer(&answer(&[7, 0, 0, 0], b"")), Some((vec![7], None)));
    }

    #[test]
    fn malformed_bodies_are_refused() {
        // shorter than its ctl, no code, no ctl, a length past the end, too long, a partial word
        assert_eq!(parse_request(&[9, 0, 0, 0, 1, 2]), None);
        assert_eq!(parse_request(&request(b"ctl", b"")), None);
        assert_eq!(parse_request(&request(b"", b"code")), None);
        assert_eq!(parse_request(&[0xff, 0xff, 0xff, 0xff, 1]), None);
        assert_eq!(parse_request(&[1, 2]), None);
        assert_eq!(parse_request(&request(b"c", &vec![0; MAX_REQUEST])), None);
        assert_eq!(parse_answer(&answer(&[1, 2, 3], b"")), None);
        assert_eq!(parse_answer(&answer(&[], b"{}")), None);
    }
}

//! The web server's shader translation route (WEB-W7b): which requests are it, the key they name,
//! the bodies it takes, and the answer it sends. The bytes are the engine's `shaders::wire`.

use crate::constants::web as c;

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    /// not this route
    Other,
    /// this route, but no shader key a file may be named by
    BadKey,
    Key {
        slug: String,
        key: String,
    },
}

/// `/<slug>/shader-xlate/<16 hex>`; the key in lower case.
pub fn route(path: &str) -> Route {
    let mut parts = path.trim_start_matches('/').split('/');
    let (Some(slug), Some(seg)) = (parts.next(), parts.next()) else {
        return Route::Other;
    };
    if seg != c::XLATE_SEGMENT || slug.is_empty() {
        return Route::Other;
    }
    match (parts.next(), parts.next()) {
        (Some(key), None) if is_key(key) => Route::Key {
            slug: slug.to_string(),
            key: key.to_ascii_lowercase(),
        },
        _ => Route::BadKey,
    }
}

/// 16 hex digits and nothing else: safe as a folder and file name.
pub fn is_key(text: &str) -> bool {
    text.len() == c::KEY_HEX_LEN && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A body of this length is read (longer ones are refused unread).
pub fn length_ok(length: u64) -> bool {
    length > 4 && length <= c::MAX_XLATE_BODY
}

/// `u32 LE ctl length | ctl | code`, with some of each.
pub fn body_ok(body: &[u8]) -> bool {
    let Some(head) = body.get(..4).and_then(|h| <[u8; 4]>::try_from(h).ok()) else {
        return false;
    };
    let ctl = u32::from_le_bytes(head) as usize;
    ctl > 0 && ctl.checked_add(4).is_some_and(|end| end < body.len())
}

/// `u32 LE spv length | spv | json`.
pub fn answer(spv: &[u8], json: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + spv.len() + json.len());
    v.extend_from_slice(&u32::try_from(spv.len()).unwrap_or(u32::MAX).to_le_bytes());
    v.extend_from_slice(spv);
    v.extend_from_slice(json);
    v
}

/// The key `nxrs-build shader-xlate` printed: its last line that is a key.
pub fn printed_key(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .rfind(|l| is_key(l))
        .map(str::to_ascii_lowercase)
}

//! Shader packs for the web build (docs/35 §4.4, WEB-W6b): the shader modules a recorded run used, in first-use
//! order, grouped into a few files the page fetches whole on the first need of any member, instead of two requests
//! per module.
//!
//! - **Pack** (`shaders/pack-<n>.bin` beside `game.js`): `NXSP`, version (u32), module count (u32), then per module its
//!   8-byte key, the SPIR-V's length (u32), the reflection json's length (u32, [`NO_JSON`] when the module has none)
//!   and the bytes. Little-endian. Not compressed itself: the servers send the build's gzip copy.
//! - **Index** (`shaders/packs.txt`): `# nxrs shader packs 1`, then a `<hex key> <pack number>` line per member.
//! - **Recording** (`shaders.rec.txt` in the data folder): the hex keys a run loaded, once each, in first-use order
//!   (`NXGFX_SHADER_REC=<file>` on a native run, graphics `vk/shader.rs`).

use std::collections::HashMap;

pub const MAGIC: &[u8; 4] = b"NXSP";
pub const VERSION: u32 = 1;
/// The json length of a module without reflection json.
pub const NO_JSON: u32 = u32::MAX;
/// The index's first line.
pub const INDEX_HEADER: &str = "# nxrs shader packs 1";
/// The folder beside `game.js` that holds the packs and their index.
pub const DIR: &str = "shaders";
/// The index's name in [`DIR`].
pub const INDEX: &str = "packs.txt";
/// The recording's name in the data folder.
pub const RECORDING: &str = "shaders.rec.txt";

/// One module of a pack: key, SPIR-V bytes, reflection json bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: [u8; 8],
    pub spv: Vec<u8>,
    pub json: Option<Vec<u8>>,
}

/// A pack's file name in [`DIR`].
pub fn pack_name(n: usize) -> String {
    format!("pack-{n:03}.bin")
}

/// A key's hex form (the library's file names).
pub fn key_hex(k: &[u8; 8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

/// A hex key (16 digits) as bytes.
pub fn parse_key(hex: &str) -> Option<[u8; 8]> {
    if hex.len() != 16 {
        return None;
    }
    let mut k = [0u8; 8];
    for (i, b) in k.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(k)
}

/// The keys of a recording, in its order, each once; lines that are not keys are skipped.
pub fn recording(text: &str) -> Vec<[u8; 8]> {
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .filter_map(|l| parse_key(l.trim()))
        .filter(|k| seen.insert(*k))
        .collect()
}

/// A pack's bytes.
pub fn encode(entries: &[Entry]) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + entries.iter().map(|e| 16 + e.spv.len()).sum::<usize>());
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&VERSION.to_le_bytes());
    v.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        v.extend_from_slice(&e.key);
        v.extend_from_slice(&(e.spv.len() as u32).to_le_bytes());
        let jl = e.json.as_ref().map_or(NO_JSON, |j| j.len() as u32);
        v.extend_from_slice(&jl.to_le_bytes());
        v.extend_from_slice(&e.spv);
        if let Some(j) = &e.json {
            v.extend_from_slice(j);
        }
    }
    v
}

/// A pack's modules; None when the bytes are not a whole pack of this version.
pub fn decode(b: &[u8]) -> Option<Vec<Entry>> {
    let u32_at = |at: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?)) };
    if b.get(..4)? != MAGIC || u32_at(4)? != VERSION {
        return None;
    }
    let n = u32_at(8)? as usize;
    let mut at = 12;
    let mut out = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let key: [u8; 8] = b.get(at..at + 8)?.try_into().ok()?;
        let sl = u32_at(at + 8)? as usize;
        let jl = u32_at(at + 12)?;
        at += 16;
        let spv = b.get(at..at.checked_add(sl)?)?.to_vec();
        at += sl;
        let json = if jl == NO_JSON {
            None
        } else {
            let j = b.get(at..at.checked_add(jl as usize)?)?.to_vec();
            at += jl as usize;
            Some(j)
        };
        out.push(Entry { key, spv, json });
    }
    (at == b.len()).then_some(out)
}

/// The groups of a recording's modules, in order, each cut when it reaches `target` bytes (a module is never split;
/// a group holds at least one).
pub fn groups(sizes: &[usize], target: usize) -> Vec<std::ops::Range<usize>> {
    let mut out = vec![];
    let mut start = 0;
    let mut bytes = 0;
    for (i, &s) in sizes.iter().enumerate() {
        bytes += s;
        if bytes >= target {
            out.push(start..i + 1);
            start = i + 1;
            bytes = 0;
        }
    }
    if start < sizes.len() {
        out.push(start..sizes.len());
    }
    out
}

/// The index text for packs of these keys.
pub fn index_text(packs: &[Vec<[u8; 8]>]) -> String {
    let mut s = String::from(INDEX_HEADER);
    s.push('\n');
    for (n, keys) in packs.iter().enumerate() {
        for k in keys {
            s.push_str(&format!("{} {n}\n", key_hex(k)));
        }
    }
    s
}

/// An index's members: key to pack number. None when the text is not an index of this version.
pub fn parse_index(text: &str) -> Option<HashMap<[u8; 8], usize>> {
    let mut lines = text.lines();
    if lines.next()?.trim() != INDEX_HEADER {
        return None;
    }
    let mut m = HashMap::new();
    for l in lines {
        let Some((k, n)) = l.trim().split_once(' ') else { continue };
        if let (Some(k), Ok(n)) = (parse_key(k), n.trim().parse()) {
            m.insert(k, n);
        }
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(k: u8, spv: &[u8], json: Option<&[u8]>) -> Entry {
        Entry { key: [k, 1, 2, 3, 4, 5, 6, 7], spv: spv.to_vec(), json: json.map(<[u8]>::to_vec) }
    }

    #[test]
    fn a_pack_reads_back_what_was_written() {
        let v = vec![e(1, &[3, 2, 3, 7], Some(b"{}")), e(2, &[1, 0, 0, 0, 9, 9, 9, 9], None), e(3, &[], Some(b""))];
        let b = encode(&v);
        assert_eq!(&b[..4], MAGIC);
        assert_eq!(decode(&b), Some(v));
    }

    #[test]
    fn a_cut_or_padded_pack_is_refused() {
        let b = encode(&[e(1, &[3, 2, 3, 7], Some(b"{\"a\":1}"))]);
        assert_eq!(decode(&b[..b.len() - 1]), None);
        let mut long = b.clone();
        long.push(0);
        assert_eq!(decode(&long), None);
        let mut other = b;
        other[4] = 2;
        assert_eq!(decode(&other), None);
    }

    #[test]
    fn the_index_names_each_member_and_its_pack() {
        let a = [0xab, 0, 0, 0, 0, 0, 0, 0x01];
        let b = [0, 0, 0, 0, 0, 0, 0, 0xff];
        let t = index_text(&[vec![a], vec![b]]);
        assert_eq!(t, "# nxrs shader packs 1\nab00000000000001 0\n00000000000000ff 1\n");
        let m = parse_index(&t).unwrap();
        assert_eq!((m[&a], m[&b], m.len()), (0, 1, 2));
        assert_eq!(parse_index("ab00000000000001 0\n"), None);
    }

    #[test]
    fn a_recording_keeps_first_use_order_once() {
        let r = recording("00000000000000ff\nzz\n\nab00000000000001\n00000000000000ff\n");
        assert_eq!(r, [[0, 0, 0, 0, 0, 0, 0, 0xff], [0xab, 0, 0, 0, 0, 0, 0, 1]]);
    }

    #[test]
    fn groups_cut_at_the_target_in_order() {
        assert_eq!(groups(&[400, 400, 400, 100, 900, 50], 1000), [0..3, 3..5, 5..6]);
        assert_eq!(groups(&[5000, 1], 1000), [0..1, 1..2]);
        assert!(groups(&[], 1000).is_empty());
    }
}

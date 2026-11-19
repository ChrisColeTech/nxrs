//! Packaged content metadata (.cnmt inside a Meta NCA).
use crate::package::error::{fmt, Result};
use crate::package::io::{hex, u16le, u32le, u64le};

pub const APPLICATION: u8 = 0x80;
pub const PATCH: u8 = 0x81;
pub const ADD_ON: u8 = 0x82;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The content's NCA id (hex); its member is `<id>.nca`.
    pub nca_id: String,
    pub size: u64,
    /// 0 meta, 1 program, 2 data, 3 control, 4 html, 5 legal, 6 delta fragment.
    pub kind: u8,
    pub id_offset: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cnmt {
    pub title_id: u64,
    pub version: u32,
    pub kind: u8,
    pub records: Vec<Record>,
}

pub fn parse(data: &[u8]) -> Result<Cnmt> {
    if data.len() < 0x20 {
        return fmt("content metadata is cut short");
    }
    let table = u16le(data, 0xe) as usize;
    let contents = u16le(data, 0x10) as usize;
    let mut records = vec![];
    for i in 0..contents {
        let at = 0x20 + table + i * 0x38;
        if at + 0x38 > data.len() {
            return fmt("content metadata record is out of range");
        }
        let mut size = [0u8; 8];
        size[..6].copy_from_slice(&data[at + 0x30..at + 0x36]);
        records.push(Record {
            nca_id: hex(&data[at + 0x20..at + 0x30]),
            size: u64::from_le_bytes(size),
            kind: data[at + 0x36],
            id_offset: data[at + 0x37],
        });
    }
    Ok(Cnmt { title_id: u64le(data, 0), version: u32le(data, 8), kind: data[0xc], records })
}

pub fn kind_name(kind: u8) -> &'static str {
    match kind {
        APPLICATION => "Application",
        PATCH => "Patch",
        ADD_ON => "AddOnContent",
        _ => "?",
    }
}

/// What dlc.py calls each title type in its refusals.
pub fn type_name(kind: u8) -> &'static str {
    match kind {
        0x80 => "the game",
        0x81 => "a game update",
        0x82 => "add-on content",
        0x83 => "an update delta",
        _ => "other content",
    }
}

/// The application id that a patch / add-on id belongs to.
pub fn base_of(title_id: u64) -> u64 {
    title_id & !0x1fff
}

/// 0100152000022000 owns 0100152000023000..37ff.
pub fn is_addon_of(title_id: u64, application: u64) -> bool {
    base_of(title_id) == base_of(application) && title_id & 0x1000 != 0
}

#[cfg(test)]
pub(crate) fn build(title_id: u64, version: u32, kind: u8, records: &[(&[u8; 16], u8)]) -> Vec<u8> {
    let mut d = vec![0u8; 0x20];
    d[..8].copy_from_slice(&title_id.to_le_bytes());
    d[8..12].copy_from_slice(&version.to_le_bytes());
    d[0xc] = kind;
    d[0xe..0x10].copy_from_slice(&0u16.to_le_bytes());
    d[0x10..0x12].copy_from_slice(&(records.len() as u16).to_le_bytes());
    for (id, t) in records {
        let mut r = vec![0u8; 0x38];
        r[0x20..0x30].copy_from_slice(&id[..]);
        r[0x30] = 0x10;
        r[0x36] = *t;
        d.extend(r);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_records() {
        let d = build(0x01001F5010DFA800, 262144, PATCH, &[(&[0xab; 16], 3)]);
        let c = parse(&d).unwrap();
        assert_eq!((c.title_id, c.version, c.kind), (0x01001F5010DFA800, 262144, PATCH));
        assert_eq!(c.records[0].nca_id, "ab".repeat(16));
        assert_eq!((c.records[0].kind, c.records[0].size), (3, 0x10));
        assert_eq!(base_of(c.title_id), 0x01001F5010DFA000);
        assert!(is_addon_of(0x0100152000023001, 0x0100152000022000));
        assert!(!is_addon_of(0x0100152000022800, 0x0100152000022000));
        assert!(parse(&d[..0x10]).is_err());
        assert!(parse(&d[..0x30]).unwrap_err().to_string().contains("record is out of range"));
    }
}

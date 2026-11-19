//! `button_layout` and `button_map`, from the owner's `pad_layout.h`. Every pad path reports its buttons by position
//! in the Switch bit order (0 = the right face button, where a Switch has A; 1 bottom (B); 2 top (X); 3 left (Y);
//! 4..15 stick clicks, L, R, ZL, ZR, Plus, Minus, D-pad left, up, right, down); this maps that positional mask to the
//! guest's buttons:
//! - `button_layout=switch` (aliases nintendo, position, positions): identity;
//! - `button_layout=xbox` (aliases label, labels): by Xbox label, bottom = A, right = B, left = X, top = Y;
//! - `button_map=south:a ...`: per-button changes on top (physical:guest pairs, separated by spaces, commas or
//!   semicolons, any case).
//!
//! The keyboard is not remapped.

pub const SWITCH: u8 = 0;
pub const XBOX: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadMap {
    /// to[physical bit] = guest bit, 0xff = none.
    pub to: [u8; 16],
    pub layout: u8,
    pub overrides: u32,
}

impl Default for PadMap {
    fn default() -> Self {
        PadMap { to: std::array::from_fn(|i| i as u8), layout: SWITCH, overrides: 0 }
    }
}

const PHYS: &[(&str, u8)] = &[
    ("east", 0),
    ("south", 1),
    ("north", 2),
    ("west", 3),
    ("circle", 0),
    ("cross", 1),
    ("triangle", 2),
    ("square", 3),
    ("lstick", 4),
    ("rstick", 5),
    ("l", 6),
    ("r", 7),
    ("zl", 8),
    ("zr", 9),
    ("plus", 10),
    ("minus", 11),
    ("left", 12),
    ("up", 13),
    ("right", 14),
    ("down", 15),
];
pub const GUEST: [&str; 16] = ["a", "b", "x", "y", "lstick", "rstick", "l", "r", "zl", "zr", "plus", "minus", "left", "up", "right", "down"];
const POS: [&str; 4] = ["right", "bottom", "top", "left"];

pub fn layout_id(v: &str) -> Option<u8> {
    match v {
        "" | "switch" | "nintendo" | "position" | "positions" => Some(SWITCH),
        "xbox" | "label" | "labels" => Some(XBOX),
        _ => None,
    }
}

fn name(s: &str, guest: bool) -> Option<u8> {
    if guest {
        if s == "none" {
            return Some(0xff);
        }
        return GUEST.iter().position(|g| *g == s).map(|i| i as u8);
    }
    PHYS.iter().find(|(n, _)| *n == s).map(|(_, b)| *b)
}

/// Builds the table. The error names the first rejected part; the count is how many were rejected (an unknown
/// layout falls back to switch, a bad entry is skipped).
pub fn build(layout: &str, map: &str) -> (PadMap, u32, String) {
    let mut bad = 0;
    let mut err = String::new();
    let id = match layout_id(layout) {
        Some(i) => i,
        None => {
            bad += 1;
            err = format!("unknown button_layout '{layout}' (switch or xbox)");
            SWITCH
        }
    };
    let mut m = PadMap { layout: id, ..Default::default() };
    if id == XBOX {
        m.to[0] = 1;
        m.to[1] = 0;
        m.to[2] = 3;
        m.to[3] = 2;
    }
    for tok in map.split([' ', ',', ';', '\t']).filter(|t| !t.is_empty()) {
        let tok = tok.to_ascii_lowercase();
        let pair = tok.split_once(':').and_then(|(p, g)| Some((name(p, false)?, name(g, true)?)));
        match pair {
            Some((p, g)) => {
                m.to[p as usize] = g;
                m.overrides += 1;
            }
            None => {
                if bad == 0 {
                    err = format!("bad button_map entry '{tok}' (physical:guest, e.g. south:a)");
                }
                bad += 1;
            }
        }
    }
    (m, bad, err)
}

pub fn apply(m: &PadMap, pos: u64) -> u64 {
    let mut out = pos & !0xffff;
    for i in 0..16 {
        if pos >> i & 1 != 0 && m.to[i] != 0xff {
            out |= 1 << m.to[i];
        }
    }
    out
}

/// "switch (A=right B=bottom X=top Y=left)" plus the overrides as physical:guest.
pub fn describe(m: &PadMap) -> String {
    let base_x = [1usize, 0, 3, 2];
    let mut s = format!("{} (", if m.layout == XBOX { "xbox" } else { "switch" });
    for g in 0..4 {
        let ph = if m.layout == XBOX { base_x[g] } else { g };
        s.push_str(&format!("{}{}={}", if g > 0 { " " } else { "" }, ["A", "B", "X", "Y"][g], POS[ph]));
    }
    s.push(')');
    if m.overrides > 0 {
        let (base, _, _) = build(if m.layout == XBOX { "xbox" } else { "switch" }, "");
        s.push_str(" button_map:");
        for i in 0..16 {
            if m.to[i] != base.to[i] {
                let ph = PHYS.iter().find(|(_, b)| *b as usize == i).map(|(n, _)| *n).unwrap_or("?");
                s.push_str(&format!(" {}:{}", ph, if m.to[i] == 0xff { "none" } else { GUEST[m.to[i] as usize] }));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        let (s, bad, _) = build("switch", "");
        assert_eq!(bad, 0);
        assert_eq!(apply(&s, 1), 1, "right is A");
        let (x, _, _) = build("xbox", "");
        assert_eq!(apply(&x, 1 << 1), 1, "bottom is A on xbox");
        assert_eq!(apply(&x, 1), 2, "right is B on xbox");
        assert_eq!(apply(&x, 1 << 3), 1 << 2, "left is X on xbox");
        assert_eq!(describe(&s), "switch (A=right B=bottom X=top Y=left)");
        assert_eq!(describe(&x), "xbox (A=bottom B=right X=left Y=top)");
    }

    #[test]
    fn overrides_and_errors() {
        let (m, bad, _) = build("switch", "South:A east:b; zl:zr,zr:zl l:none");
        assert_eq!(bad, 0);
        assert_eq!(apply(&m, 1 << 1), 1);
        assert_eq!(apply(&m, 1 << 8), 1 << 9);
        assert_eq!(apply(&m, 1 << 6), 0, "mapped to none");
        assert_eq!(apply(&m, 1 << 20), 1 << 20, "bits past 15 pass");
        assert!(describe(&m).contains("button_map: east:b south:a l:none zl:zr zr:zl"));
        let (m, bad, err) = build("gamecube", "south:q nonsense cross:b");
        assert_eq!(bad, 3);
        assert!(err.contains("gamecube"));
        assert_eq!(m.layout, SWITCH);
        assert_eq!(apply(&m, 1 << 1), 1 << 1);
    }
}

//! Execution hints: `discovered.tsv`, the code a running game reached that the build had not translated (the
//! runtime's interpreter ran it and wrote it down: `nxrt::interp`). Each entry becomes a function start at the next
//! build of the same program, when the module still has the entry's instruction words there; an entry of another
//! version of the program (an update applied or taken back) is left out, not misplaced (another version's hints can
//! stop a game at start).
//!
//! Format, one entry a line (`#` comments): `module \t 0xoffset \t word word word word` (hex words, the first
//! instructions at the entry as the game had them).

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub offset: u64,
    pub words: Vec<u32>,
}

pub const FILE: &str = "discovered.tsv";

/// The entries of a discovered.tsv text by module (comments and malformed lines skipped, duplicates once).
pub fn parse(text: &str) -> BTreeMap<String, Vec<Hint>> {
    let mut out: BTreeMap<String, Vec<Hint>> = BTreeMap::new();
    for l in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let mut f = l.split('\t');
        let (Some(module), Some(off)) = (f.next(), f.next()) else {
            continue;
        };
        let Ok(offset) = u64::from_str_radix(off.trim().trim_start_matches("0x"), 16) else {
            continue;
        };
        let words = f
            .next()
            .unwrap_or("")
            .split_whitespace()
            .filter_map(|w| u32::from_str_radix(w, 16).ok())
            .collect();
        let v = out.entry(module.trim().to_string()).or_default();
        if !v.iter().any(|h| h.offset == offset) {
            v.push(Hint { offset, words });
        }
    }
    out
}

/// The hints of one module that hold: aligned, inside its code, with the same words there (`word(a)` reads the
/// module's code at guest address a). Returns (absolute starts kept, entries left out).
pub fn usable(
    hints: &[Hint],
    base: u64,
    lo: u64,
    hi: u64,
    word: &dyn Fn(u64) -> u32,
) -> (Vec<u64>, usize) {
    let mut kept = vec![];
    let mut dropped = 0;
    for h in hints {
        let a = base.wrapping_add(h.offset);
        let fits = a % 4 == 0 && lo <= a && a < hi;
        let same = fits
            && !h.words.is_empty()
            && h.words.iter().enumerate().all(|(i, &w)| {
                let x = a + 4 * i as u64;
                x < hi && word(x) == w
            });
        if same {
            kept.push(a);
        } else {
            dropped += 1;
        }
    }
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_check_words() {
        let t = "# header\nmain\t0x10\taaaa0000 bbbb0000\nmain\t0x10\taaaa0000\nsdk\t0x4\t11111111\nbad line\n";
        let h = parse(t);
        assert_eq!(
            h["main"],
            vec![Hint {
                offset: 0x10,
                words: vec![0xaaaa0000, 0xbbbb0000]
            }]
        );
        assert_eq!(h["sdk"].len(), 1);
        let code = [0u32, 0, 0, 0, 0xaaaa0000, 0xbbbb0000, 0, 0];
        let base = 0x1000u64;
        let word = |a: u64| code[((a - base) / 4) as usize];
        let (kept, dropped) = usable(&h["main"], base, base, base + 32, &word);
        assert_eq!((kept, dropped), (vec![0x1010], 0));
        // another version: other words there
        let (kept, dropped) = usable(&h["sdk"], base, base, base + 32, &word);
        assert_eq!((kept.len(), dropped), (0, 1));
        // outside the code
        let far = vec![Hint {
            offset: 0x100,
            words: vec![0],
        }];
        assert_eq!(usable(&far, base, base, base + 32, &word).1, 1);
    }
}

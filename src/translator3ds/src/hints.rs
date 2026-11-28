//! the interpreter's misses fed back as function seeds: addresses a previous run had to interpret,
//! one address per line, bit 0 set for Thumb, keyed by title and code hash so one revision's hints stay
//! out of another's. The donor carries this in build.rs; here the hints come in as a parameter (where
//! they're read from is the pipeline's, 3DS-W1, and the write side is runtime3ds's miss loop, 3DS-W2).

use crate::discover::{self, Analysis, Mode, Program, Source};

/// takes in where the last run had to interpret the title's lib (it only ever ran code)
/// spots outside the known funcs start new ones, found by analyzing the exe again, returns how many
/// a spot inside one adds nothing, discovery already labels every way in (branch targets, returns from calls and svcs)
/// the gathered list the bundled half reads rides a later wave (the runtime's miss recorder, 3DS-W2)
pub fn apply_hints(
    extra: &[u32],
    programs: &mut [(String, Program)],
    analyses: &mut [Analysis],
) -> usize {
    let Some(index) = programs.iter().position(|(name, _)| name == "executable") else {
        return 0;
    };
    let program = &mut programs[index].1;
    let text = program.text.base..program.text.end();
    let mut hinted = extra.to_vec();
    hinted.retain(|&address| text.contains(&(address & !1)));
    hinted.sort_unstable();
    hinted.dedup();
    if hinted.is_empty() {
        return 0;
    }
    let outside = new_functions(&analyses[index], &hinted);
    if !outside.is_empty() {
        // plus the hints the first pass already starts a func at in their mode, the second pass goes its own way
        // and a guess could grab their spot (alpha sapphire's 0x500000 flipped from arm to a thumb pointer)
        let analysis = &analyses[index];
        let starts = hinted.iter().copied().filter(|&address| {
            analysis
                .functions
                .get(&(address & !1))
                .is_some_and(|f| (f.mode == Mode::Thumb) == (address & 1 != 0))
        });
        let seeds: Vec<u32> = outside.iter().copied().chain(starts).collect();
        program
            .seeds
            .extend(seeds.into_iter().map(|address| (address, Source::Hint)));
        analyses[index] = discover::analyze(program);
    }
    outside.len()
}

/// hinted spots no found func holds, they start their own funcs, odd for Thumb
pub fn new_functions(analysis: &Analysis, hinted: &[u32]) -> Vec<u32> {
    let mut owned = std::collections::HashSet::new();
    for function in analysis.functions.values() {
        let thumb = (function.mode == Mode::Thumb) as u32;
        owned.extend(function.instructions.iter().map(|&address| address | thumb));
    }
    hinted
        .iter()
        .copied()
        .filter(|address| !owned.contains(address))
        .collect()
}

/// addrs in a hints file, one address per line, bit 0 set for Thumb
pub fn read_hints(text: &str) -> Vec<u32> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| u32::from_str_radix(line.trim_start_matches("0x"), 16).ok())
        .collect()
}

/// addrs under a list's header for a title's code; sections start `= <program id> <code hash>`
/// the gathered list rides a later wave (the runtime's miss recorder fills it)
pub fn bundled(list: &str, program_id: u64, hash: u64) -> Vec<u32> {
    section(list, program_id, hash)
}

/// hash of a title's exe code (FNV1a over its bytes) so one revision's hints stay out of another's
pub fn code_hash(program: &Program) -> u64 {
    program
        .text
        .bytes
        .iter()
        .fold(0xCBF2_9CE4_8422_2325, |hash, &byte| {
            (hash ^ byte as u64).wrapping_mul(0x0100_0000_01B3)
        })
}

/// addrs under a list's header for a title's code
fn section(list: &str, program_id: u64, hash: u64) -> Vec<u32> {
    let mut taking = false;
    let mut found = Vec::new();
    for line in list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if let Some(header) = line.strip_prefix('=') {
            let mut fields = header
                .split_whitespace()
                .map(|field| u64::from_str_radix(field, 16).ok());
            taking = fields.next().flatten() == Some(program_id)
                && fields.next().flatten() == Some(hash);
        } else if taking {
            found.extend(u32::from_str_radix(line.trim_start_matches("0x"), 16).ok());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Segment;

    fn program(words: &[u32]) -> Program {
        Program {
            text: Segment {
                base: 0x0010_0000,
                bytes: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
            },
            seeds: vec![(0x0010_0000, Source::Entry)],
            slots: None,
        }
    }

    /// a game's hints in the list go to the code they were gathered on, a revision or update w/ other code gets none
    #[test]
    fn the_list_gives_hints_to_the_code_they_are_for() {
        let list = "# a note\n= 0004000000033500 00000000000000AB\n00100000\n00100011\n\n= 0004000000033500 00000000000000AC\n00200000\n";
        assert_eq!(
            section(list, 0x0004_0000_0003_3500, 0xAB),
            [0x0010_0000, 0x0010_0011]
        );
        assert_eq!(section(list, 0x0004_0000_0003_3500, 0xAC), [0x0020_0000]);
        assert!(section(list, 0x0004_0000_0003_3600, 0xAB).is_empty());
        assert!(section(list, 0x0004_0000_0003_3500, 0xAD).is_empty());
    }

    /// a hints file reads, one address per line, comments and blanks skipped, odd means Thumb
    #[test]
    fn a_hints_file_reads() {
        let text = "# gathered on the last run\n0x00100000\n00100011\n\n  # note\n";
        assert_eq!(read_hints(text), [0x0010_0000, 0x0010_0011]);
    }

    /// a hint outside every found func starts one, odd for Thumb, and a hint inside a func adds nothing
    #[test]
    fn a_hint_outside_every_func_starts_one() {
        // bx lr, then mov r0, 1 behind a conditional branch, only reachable where a hint says
        // (an unconditional word would start the gap scan's own func there and the hint would add nothing)
        let fixture = program(&[0xE12F_FF1E, 0x1A00_0000, 0xE3A0_0001, 0xE12F_FF1E]);
        let analysis = discover::analyze(&fixture);
        assert_eq!(analysis.functions.len(), 1);
        let hash = code_hash(&fixture);
        assert_eq!(
            bundled(&format!("= 0 {hash:016X}\n00100005\n"), 0, hash),
            [0x0010_0005]
        );

        let mut programs = vec![("executable".to_owned(), fixture)];
        let mut analyses = vec![discover::analyze(&programs[0].1)];
        let added = apply_hints(&[0x0010_0005], &mut programs, &mut analyses);
        assert_eq!(added, 1);
        let analysis = &analyses[0];
        assert!(analysis.functions.contains_key(&0x0010_0004));
        assert_eq!(analysis.functions[&0x0010_0004].mode, Mode::Thumb);
        assert!(programs[0]
            .1
            .seeds
            .iter()
            .any(|&(address, source)| (address, source) == (0x0010_0005, Source::Hint)));
        // a hint at the entry's own code is already owned, no new func
        let mut programs = vec![(
            "executable".to_owned(),
            program(&[0xE12F_FF1E, 0x1A00_0000, 0xE3A0_0001, 0xE12F_FF1E]),
        )];
        let mut analyses = vec![discover::analyze(&programs[0].1)];
        assert_eq!(apply_hints(&[0x0010_0000], &mut programs, &mut analyses), 0);
    }
}

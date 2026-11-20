//! Atmosphère cheat files as both the build and the run read them: the entries (`[Name]` a cheat, `{Name}` a master
//! code, 8-hex-digit code words; an entry without code words is a label and left out; a header glued to the previous
//! line's last word still starts a new entry), and, for the build, the writes a cheat makes into main's code.
//!
//! Code cheats (risk R2): the game runs code translated at build time, so a cheat that writes main's instructions
//! changes bytes the game never executes. The build compiles such writes in as a switchable patch set per cheat (the
//! mods' site machinery: both the original and the patched instruction are lifted); the set's switch is the cheat's
//! toggle at run time (os `cheats.rs` flips it through the runtime's patch table). A cheat qualifies when every
//! instruction is opcode 0 (store a static value) into main (memory type 0), with the offset register taken as 0
//! (the registers start each pass at 0); a cheat with any logic stays the run-time VM's alone.

/// One entry of a cheat file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub master: bool,
    pub words: Vec<u32>,
}

/// The entries with code words, in file order.
pub fn entries(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = vec![];
    let mut cur: Option<Entry> = None;
    for raw in text.lines() {
        let mut line = raw.trim();
        while !line.is_empty() {
            if line.starts_with('[') || line.starts_with('{') {
                let close = if line.starts_with('[') { ']' } else { '}' };
                let Some(end) = line.find(close) else { break };
                if let Some(c) = cur.take() {
                    if !c.words.is_empty() {
                        out.push(c);
                    }
                }
                cur = Some(Entry { name: line[1..end].trim().to_string(), master: close == '}', words: vec![] });
                line = line[end + 1..].trim_start();
                continue;
            }
            let tok_end = line.find(|c: char| c.is_whitespace() || c == '[' || c == '{').unwrap_or(line.len());
            let tok = &line[..tok_end];
            if tok.len() == 8 && tok.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Some(c) = cur.as_mut() {
                    c.words.push(u32::from_str_radix(tok, 16).unwrap());
                }
            } else {
                break; // prose (a credits line): ignored
            }
            line = line[tok_end..].trim_start();
        }
    }
    if let Some(c) = cur.take() {
        if !c.words.is_empty() {
            out.push(c);
        }
    }
    out
}

/// The run-time switch of a cheat's patch set: `NXRS_CHEAT_<FNV-1a 32 of its name>` (the build and the run name it
/// alike; a second entry of the same name gets `_2`, `_3`...).
pub fn env_name(name: &str, nth: usize) -> String {
    let h = name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    if nth <= 1 {
        format!("NXRS_CHEAT_{h:08X}")
    } else {
        format!("NXRS_CHEAT_{h:08X}_{nth}")
    }
}

/// Every entry's switch, in file order (the occurrence count of its name kept).
pub fn env_names(es: &[Entry]) -> Vec<String> {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    es.iter()
        .map(|e| {
            let n = seen.entry(e.name.as_str()).or_insert(0);
            *n += 1;
            env_name(&e.name, *n)
        })
        .collect()
}

/// The writes of a cheat made only of opcode-0 stores into main: (offset in main's image, bytes, little-endian);
/// None for any other cheat.
pub fn main_writes(words: &[u32]) -> Option<Vec<(u64, Vec<u8>)>> {
    let mut out = vec![];
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let (op, t, m) = (w >> 28, (w >> 24) & 0xF, (w >> 20) & 0xF);
        if op != 0 || m != 0 || !matches!(t, 1 | 2 | 4 | 8) {
            return None;
        }
        let n = if t == 8 { 4 } else { 3 };
        if i + n > words.len() {
            return None;
        }
        let rel = ((w & 0xFF) as u64) << 32 | words[i + 1] as u64;
        let v = if t == 8 { (words[i + 2] as u64) << 32 | words[i + 3] as u64 } else { words[i + 2] as u64 };
        out.push((rel, v.to_le_bytes()[..t as usize].to_vec()));
        i += n;
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_and_main_writes() {
        let t = "[Credits: someone]\n{Master}\n04000000 00000100 00000001\n[Moon jump]\n580F0000 01234567\n[Label only]\n[Two]\n04000000 00001000 D503201F 02000000 00001006 00001234\n[Wide]\n08000000 00002000 11111111 22222222\n";
        let e = entries(t);
        assert_eq!(e.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Master", "Moon jump", "Two", "Wide"]);
        assert!(e[0].master);
        assert_eq!(main_writes(&e[1].words), None, "a pointer cheat stays the VM's");
        assert_eq!(main_writes(&e[2].words), Some(vec![(0x1000, vec![0x1f, 0x20, 0x03, 0xd5]), (0x1006, vec![0x34, 0x12])]));
        assert_eq!(main_writes(&e[3].words), Some(vec![(0x2000, vec![0x22, 0x22, 0x22, 0x22, 0x11, 0x11, 0x11, 0x11])]));
        assert_eq!(main_writes(&[0x0410_0000, 0, 1]), None, "the heap is not main");
        let n = env_names(&[e[2].clone(), e[2].clone()]);
        assert_eq!(n[0], env_name("Two", 1));
        assert!(n[1].ends_with("_2") && n[0].starts_with("NXRS_CHEAT_") && n[0].len() == 19);
    }
}

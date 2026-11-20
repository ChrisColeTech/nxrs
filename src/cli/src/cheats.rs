//! Code cheats compiled in (risk R2): the cheat file of main's build (`<data>\cheats\<build>.txt`, or the Atmosphère
//! tree `<data>\cheats\atmosphere\contents\<title>\cheats\<build>.txt`; `NXOS_CHEATS=<folder>` names another, as at
//! run time) is read at build time, and every cheat whose writes are plain stores into main (`profiles::cheatfile`)
//! has the words it writes into main's code added to the patch table as a set of its own: `cheat_<hash>`, switch
//! `NXRS_CHEAT_<hash>`, off by default (a master code: on). os `cheats.rs` flips the set when the cheat is toggled
//! (the game's code hands its switch table to the runtime). Writes outside main's code stay the run-time VM's.

use crate::out::Out;
use profiles::cheatfile;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The cheat folder of a build's data (`NXOS_CHEATS` overrides it, as at run time).
fn folder(data: &Path) -> PathBuf {
    std::env::var_os("NXOS_CHEATS").map(PathBuf::from).unwrap_or_else(|| data.join("cheats"))
}

/// The program id in main.npdm (META: ACI0 offset at +0x70; ACI0: program id at +0x10).
fn program_id(npdm: &[u8]) -> Option<u64> {
    if npdm.len() < 0x80 || &npdm[..4] != b"META" {
        return None;
    }
    let aci = u32::from_le_bytes(npdm[0x70..0x74].try_into().ok()?) as usize;
    let b = npdm.get(aci..aci + 0x18)?;
    (&b[..4] == b"ACI0").then(|| u64::from_le_bytes(b[0x10..0x18].try_into().unwrap()))
}

/// The table lines (`set` / `site`) for the code cheats of `text` over main's `image` (its text at `text_range`);
/// `taken` holds the words the profile or a mod patches already (left out, said). Returns (lines, cheats compiled in).
pub fn table(text: &str, image: &[u8], text_range: (usize, usize), taken: &mut BTreeSet<u64>, out: &Out) -> (String, usize) {
    let es = cheatfile::entries(text);
    let envs = cheatfile::env_names(&es);
    let (t0, t1) = (text_range.0 as u64, (text_range.0 + text_range.1) as u64);
    let mut s = String::new();
    let mut n = 0;
    for (e, env) in es.iter().zip(&envs) {
        let Some(writes) = cheatfile::main_writes(&e.words) else { continue };
        let mut patched = image.to_vec();
        let mut words: BTreeSet<u64> = BTreeSet::new();
        for (off, bytes) in &writes {
            for (k, b) in bytes.iter().enumerate() {
                let a = off + k as u64;
                if (t0..t1).contains(&a) && (a as usize) < patched.len() {
                    patched[a as usize] = *b;
                    words.insert(a & !3);
                }
            }
        }
        let mut sites = vec![];
        let set = env.trim_start_matches("NXRS_CHEAT_").to_ascii_lowercase();
        for w in words {
            let rd = |v: &[u8]| u32::from_le_bytes(v[w as usize..w as usize + 4].try_into().unwrap());
            let (o, r) = (rd(image), rd(&patched));
            if o == r {
                continue;
            }
            if !taken.insert(w) {
                out.say(&format!("  cheat {}: main+{w:#x} is patched already (the profile or a mod); this word is left out", e.name));
                continue;
            }
            sites.push(format!("site cheat_{set} main {w:#x} {o:08x} {r:08x}  # cheat {}", e.name.replace('#', "")));
        }
        if sites.is_empty() {
            continue;
        }
        s.push_str(&format!("set cheat_{set} {env} {}\n", e.master as u8));
        for x in &sites {
            s.push_str(x);
            s.push('\n');
        }
        out.say(&format!("  cheat {}: {} instruction words of main compiled in, switched by the cheat's toggle ({env})", e.name, sites.len()));
        n += 1;
    }
    (s, n)
}

/// Adds the code cheats of main's build to the patch table (returned).
pub fn prepare(code: &BTreeMap<String, Vec<u8>>, data: &Path, profile_table: Option<String>, out: &Out) -> anyhow::Result<Option<String>> {
    let Some(main) = code.get("main").and_then(|raw| translator::loader::Nso::parse(raw).ok()) else { return Ok(profile_table) };
    let mut build: String = main.build_id.chars().take(16).collect::<String>().to_ascii_uppercase();
    while build.len() < 16 {
        build.push('0');
    }
    let dir = folder(data);
    let mut candidates = vec![dir.join(format!("{build}.txt"))];
    if let Some(tid) = code.get("main.npdm").and_then(|n| program_id(n)) {
        candidates.push(dir.join("atmosphere").join("contents").join(format!("{tid:016X}")).join("cheats").join(format!("{build}.txt")));
    }
    let Some(file) = candidates.into_iter().find(|p| p.is_file()) else { return Ok(profile_table) };
    let text = std::fs::read_to_string(&file)?;
    // words patched already: the profile's and the mods' sites on main
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    for l in profile_table.as_deref().unwrap_or("").lines() {
        let f: Vec<&str> = l.split('#').next().unwrap_or("").split_whitespace().collect();
        if f.len() >= 6 && f[0] == "site" && f[2] == "main" {
            if let Ok(o) = u64::from_str_radix(f[3].trim_start_matches("0x"), 16) {
                taken.insert(o);
            }
        }
    }
    out.say(&format!("Cheats: {}", file.display()));
    let (lines, n) = table(&text, &main.image, main.text, &mut taken, out);
    if n == 0 {
        out.say("  no code cheat (writes into main's code) to compile in; data cheats run as they are");
        return Ok(profile_table);
    }
    let mut t = profile_table.unwrap_or_default();
    t.push_str("# The code cheats of main's build (nxrs-build cheats.rs).\n");
    t.push_str(&lines);
    Ok(Some(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cheat that NOPs one instruction of main and writes half of another becomes a set of its own with one site
    /// per changed word (original and patched words from the image); a word the profile patches is left out; a
    /// pointer cheat and a write into main's data add nothing; a master's set is on by default.
    #[test]
    fn code_cheats_become_patch_sets() {
        let mut image = vec![0u8; 0x3000];
        for (i, w) in [0x9400_0010u32, 0xF900_0020, 0x5280_0000].iter().enumerate() {
            image[0x1000 + 4 * i..0x1004 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        let text = "[No hit]\n04000000 00001000 D503201F\n02000000 00001006 00001234\n[Pointer]\n580F0000 01234567\n[Data]\n04000000 00002800 00000001\n{Master}\n04000000 00001008 52800020\n";
        let mut taken: BTreeSet<u64> = BTreeSet::from([0x1004]);
        let (t, n) = table(text, &image, (0x1000, 0x1000), &mut taken, &Out::new(true, false));
        assert_eq!(n, 2, "{t}");
        let env = cheatfile::env_name("No hit", 1);
        let set = format!("cheat_{}", env.trim_start_matches("NXRS_CHEAT_").to_ascii_lowercase());
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], format!("set {set} {env} 0"));
        assert!(lines[1].starts_with(&format!("site {set} main 0x1000 94000010 d503201f")), "{}", lines[1]);
        assert_eq!(lines.iter().filter(|l| l.contains("0x1004")).count(), 0, "the profile's word is left out");
        assert!(lines.iter().any(|l| l.starts_with("set cheat_") && l.ends_with(" 1")), "the master is on by default");
        assert!(lines.iter().any(|l| l.contains("main 0x1008 52800000 52800020")));
    }
}

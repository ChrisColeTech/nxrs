//! The player's mods in a build (`profiles::mods` reads them): code patches become the translator's switchable sites
//! (both the original and the patched instruction are lifted; a patch set per mod, on by default, `NXRS_MOD_<NAME>=0`
//! or a `disabled` file in the mod's folder turns it off at run time, `=1` forces it on), patches of data bytes and
//! the RomFS layer go to `<data>\mods.tsv` for the run (src/os `mods.rs`). Mods live in `<build folder>\mods\<mod>\`
//! (`NXRS_MODS=<folder>` names another). They are the player's files: nxrs never ships or downloads one.

use crate::out::Out;
use anyhow::{bail, Context, Result};
use profiles::mods::{self, Edit, Mod};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// (switch, folder) of every mod with code sites: modules.c checks the `disabled` markers by them.
static C_DIRS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The mods folder of a build.
pub fn mods_dir(outdir: &Path) -> PathBuf {
    std::env::var_os("NXRS_MODS").map(PathBuf::from).unwrap_or_else(|| outdir.join("mods"))
}

struct Image {
    image: Vec<u8>,
    text: (usize, usize),
    build_id: String,
}

/// Reads the mods, adds their sets and sites to the profile's patch table (returned), and writes `<data>\mods.tsv`
/// (or removes it when there are no mods).
pub fn prepare(outdir: &Path, code: &BTreeMap<String, Vec<u8>>, data: &Path, profile_table: Option<String>, out: &Out) -> Result<Option<String>> {
    let dir = mods_dir(outdir);
    let found = mods::scan(&dir);
    let tsv = data.join("mods.tsv");
    C_DIRS.lock().unwrap().clear();
    if found.is_empty() {
        let _ = std::fs::remove_file(&tsv);
        return Ok(profile_table);
    }
    if found.len() > 200 {
        bail!("{}: {} mods; at most 200 patch sets fit", dir.display(), found.len());
    }
    let mut images: BTreeMap<String, Image> = BTreeMap::new();
    for (name, raw) in code {
        if name.ends_with(".npdm") {
            continue;
        }
        if let Ok(n) = translator::loader::Nso::parse(raw) {
            images.insert(name.clone(), Image { image: n.image.clone(), text: n.text, build_id: mods::norm_build_id(&n.build_id) });
        }
    }
    // words the profile already patches: a mod's site there would be a second patch of one word
    let mut taken: BTreeSet<(String, u64)> = BTreeSet::new();
    for l in profile_table.as_deref().unwrap_or("").lines() {
        let f: Vec<&str> = l.split('#').next().unwrap_or("").split_whitespace().collect();
        if f.len() >= 6 && f[0] == "site" {
            if let Ok(o) = u64::from_str_radix(f[3].trim_start_matches("0x"), 16) {
                taken.insert((f[2].to_string(), o));
            }
        }
    }
    let mut table = String::new();
    let mut lines = vec![String::from("# Written by nxrs-build: the mods of this build (src/os mods.rs reads it). Do not edit.")];
    for m in &found {
        report(m, out);
        let (set, env) = (m.set_name(), m.env());
        let mut sites = vec![];
        for pf in &m.patches {
            let Some((mname, img)) = images.iter().find(|(_, i)| i.build_id == pf.build_id) else {
                out.say(&format!("  mod {}: {} is for build {}, which no module of this game has; left out", m.name, pf.source, pf.build_id));
                continue;
            };
            let edits: Vec<&Edit> = pf.edits.iter().collect();
            let patched = mods::apply(&img.image, &edits);
            let (t0, t1) = (img.text.0 as u64, (img.text.0 + img.text.1) as u64);
            let mut words: BTreeSet<u64> = BTreeSet::new();
            for e in &edits {
                for k in 0..e.bytes.len() as u64 {
                    let a = e.offset + k;
                    if a >= img.image.len() as u64 {
                        out.say(&format!("  mod {}: {} writes past the end of {mname} ({a:#x}); that byte is left out", m.name, pf.source));
                        break;
                    }
                    if (t0..t1).contains(&a) {
                        words.insert(a & !3);
                    } else {
                        lines.push(format!("bytes\t{env}\t{mname}\t{a:#x}\t{:02x}", patched[a as usize]));
                    }
                }
            }
            for w in words {
                let rd = |v: &[u8]| u32::from_le_bytes(v[w as usize..w as usize + 4].try_into().unwrap());
                let (o, r) = (rd(&img.image), rd(&patched));
                if o == r {
                    continue;
                }
                if !taken.insert((mname.clone(), w)) {
                    out.say(&format!("  mod {}: {mname}+{w:#x} is patched already (the profile or another mod); this word is left out", m.name));
                    continue;
                }
                sites.push(format!("site {set} {mname} {w:#x} {o:08x} {r:08x}  # mod {}", m.name.replace('#', "")));
            }
        }
        if !sites.is_empty() {
            table.push_str(&format!("set {set} {env} {}\n", m.default_on as u8));
            for s in &sites {
                table.push_str(s);
                table.push('\n');
            }
            C_DIRS.lock().unwrap().push((env.clone(), m.folder.clone()));
        }
        let romfs = m.romfs.as_ref().map_or("-".to_string(), |p| std::path::absolute(p).unwrap_or(p.clone()).display().to_string());
        lines.insert(1, format!("mod\t{env}\t{}\t{}\t{romfs}", m.default_on as u8, m.folder));
        out.say(&format!("  mod {}: {} instruction words switchable ({env})", m.name, sites.len()));
    }
    std::fs::create_dir_all(data).with_context(|| format!("{}", data.display()))?;
    crate::write_if_changed(&tsv, (lines.join("\n") + "\n").as_bytes())?;
    if table.is_empty() {
        return Ok(profile_table);
    }
    let mut t = profile_table.unwrap_or_default();
    t.push_str("# The player's mods (nxrs-build mods.rs).\n");
    t.push_str(&table);
    Ok(Some(t))
}

fn report(m: &Mod, out: &Out) {
    out.say(&format!(
        "Mod: {} ({}): {} patch file(s){}{}",
        m.name,
        m.dir.display(),
        m.patches.len(),
        if m.romfs.is_some() { ", RomFS files layered over the game's" } else { "" },
        if m.default_on { "" } else { ", off by default (mod.json)" }
    ));
    for p in &m.problems {
        out.say(&format!("  mod {}: {p}", m.name));
    }
}

/// modules.c's check of the mods' `disabled` markers (`<exe folder>\mods\<folder>\disabled`): after the switches,
/// before the game starts; a set whose environment switch is given is left as that says.
pub fn c_code() -> String {
    let dirs = C_DIRS.lock().unwrap().clone();
    let mut s = String::from("static void nxrs_mod_markers(void) {\n");
    if dirs.is_empty() {
        s.push_str("}\n\n");
        return s;
    }
    s.push_str("    static const char *dirs[][2] = {\n");
    for (env, folder) in &dirs {
        let esc: String = folder.chars().flat_map(|c| if c == '"' || c == '\\' { vec!['\\', c] } else { vec![c] }).collect();
        s.push_str(&format!("        {{\"{env}\", \"{esc}\"}},\n"));
    }
    if !cfg!(windows) {
        // Linux: the executable's folder from /proc/self/exe, '/' separators, stat() for the marker
        s.push_str(
            "    };\n\
             \x20   char exe[1024];\n\
             \x20   long n = readlink(\"/proc/self/exe\", exe, sizeof exe - 1);\n\
             \x20   if (n <= 0) return;\n\
             \x20   while (n > 0 && exe[n - 1] != '/') n--;\n\
             \x20   exe[n] = 0;\n\
             #define NXRS_MARK(i, name, env, def) { const char *v = getenv(env); if (!(v && v[0])) for (unsigned k = 0; k < sizeof dirs / sizeof dirs[0]; k++) if (!strcmp(dirs[k][0], env)) { char p[1400]; struct stat st; snprintf(p, sizeof p, \"%smods/%s/disabled\", exe, dirs[k][1]); if (stat(p, &st) == 0) rt_patch_on[i] = 0; } }\n\
             \x20   RT_PATCH_SETS(NXRS_MARK)\n\
             #undef NXRS_MARK\n\
             }\n\n",
        );
        return s;
    }
    s.push_str(
        "    };\n\
         \x20   char exe[1024];\n\
         \x20   unsigned long n = GetModuleFileNameA(0, exe, sizeof exe);\n\
         \x20   if (n == 0 || n >= sizeof exe) return;\n\
         \x20   while (n > 0 && exe[n - 1] != '\\\\') n--;\n\
         \x20   exe[n] = 0;\n\
         #define NXRS_MARK(i, name, env, def) { const char *v = getenv(env); if (!(v && v[0])) for (unsigned k = 0; k < sizeof dirs / sizeof dirs[0]; k++) if (!strcmp(dirs[k][0], env)) { char p[1400]; snprintf(p, sizeof p, \"%smods\\\\%s\\\\disabled\", exe, dirs[k][1]); if (GetFileAttributesA(p) != 0xFFFFFFFFu) rt_patch_on[i] = 0; } }\n\
         \x20   RT_PATCH_SETS(NXRS_MARK)\n\
         #undef NXRS_MARK\n\
         }\n\n",
    );
    s
}

/// The declarations c_code needs.
#[cfg(windows)]
pub const C_DECLS: &str = "#include <stdio.h>\n#include <string.h>\n\
__declspec(dllimport) unsigned long __stdcall GetModuleFileNameA(void *module, char *name, unsigned long size);\n\
__declspec(dllimport) unsigned long __stdcall GetFileAttributesA(const char *name);\n";
/// The declarations c_code needs (Linux: readlink and stat).
#[cfg(not(windows))]
pub const C_DECLS: &str = "#include <stdio.h>\n#include <string.h>\n#include <stdlib.h>\n#include <unistd.h>\n#include <sys/stat.h>\n";

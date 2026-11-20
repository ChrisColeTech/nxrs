//! The game profile in the build (`Profile:` line, the patch table the translator applies) and `nxrs-build profile`.

use crate::out::Out;
use anyhow::{anyhow, Result};
use profiles::{How, Match};
use std::collections::BTreeMap;
use std::path::Path;

/// (module, build id) of every ExeFS module, build ids as lowercase hex.
pub fn module_builds(exefs: &BTreeMap<String, Vec<u8>>) -> Vec<(String, String)> {
    exefs
        .iter()
        .filter_map(|(n, d)| translator::package::nso_build_id(d).map(|b| (n.clone(), b.iter().map(|x| format!("{x:02x}")).collect())))
        .collect()
}

pub fn find(title_id: Option<u64>, mods: &[(String, String)]) -> Option<Match> {
    let main = mods.iter().find(|(n, _)| n == "main").map(|(_, b)| b.as_str());
    profiles::lookup(title_id, main)
}

/// The build's one line about its profile.
pub fn line(m: &Option<Match>, title_id: Option<u64>, mods: &[(String, String)]) -> String {
    let main = mods.iter().find(|(n, _)| n == "main").map(|(_, b)| b.as_str()).unwrap_or("?");
    match m {
        Some(Match { profile: p, build: Some(b), how: How::Build }) => {
            format!("Profile: {} {} (build {} = {}, counter {} Hz)", p.id, p.name, b.version, b.display, p.tick_hz())
        }
        Some(Match { profile: p, .. }) => format!(
            "Profile: {} {}: main build {main} is not one it registers (its tables are of {}); its memory layout and settings apply",
            p.id,
            p.name,
            p.package.as_deref().unwrap_or("another version")
        ),
        None => format!(
            "Profile: none for title {} (main build {main}); defaults apply",
            title_id.map_or("unknown".into(), |t| format!("{t:016X}"))
        ),
    }
}

/// The translator's patch table for this program, or None (also with NXRS_NO_PROFILE_PATCHES=1).
pub fn patch_table(mods: &[(String, String)], out: &Out) -> Result<Option<String>> {
    if std::env::var("NXRS_NO_PROFILE_PATCHES").is_ok_and(|v| !v.is_empty() && v != "0") {
        out.say("Profile patches: left out (NXRS_NO_PROFILE_PATCHES)");
        return Ok(None);
    }
    let refs: Vec<(&str, &str)> = mods.iter().map(|(n, b)| (n.as_str(), b.as_str())).collect();
    let (text, applied) = profiles::translator_patches(&refs).map_err(|e| anyhow!("profile patches: {e}"))?;
    for r in &applied {
        out.say(&format!("patch applied: {}", r.split(". ").next().unwrap_or(r)));
    }
    Ok((!text.is_empty()).then_some(text))
}

/// Purposes of nxnative's patch rules that were made for its platform (Apple's address space limits), not for the
/// game: nxrs's guest space is 512 GB, so these stay out.
const PLATFORM_PURPOSES: &[&str] = &["address space"];

/// nxnative's MATCH cuts (`[[match]] kind = "cuts"`) placed by function fingerprint (`translator::aot::nso_match`) in
/// this build's modules: the selected profile's rules for a module whose build registers no cuts of its own (nxnative's
/// rule), and the SDK- / engine-wide rules for any game's module of that name. Rules whose binding names a platform
/// purpose stay out. Returns the sites for the translator's table (set `match`, `NXRS_PATCH_MATCH=0` turns them off)
/// and one line per rule.
pub fn match_sites(prof: &Option<Match>, exefs: &BTreeMap<String, Vec<u8>>, builds: &[(String, String)]) -> (Vec<String>, Vec<String>) {
    use translator::aot::nso_match::Image;
    let (mut sites, mut said) = (vec![], vec![]);
    let own: Vec<&profiles::patches::PatchFile> = prof.as_ref().map(|m| m.profile.patch_files().collect()).unwrap_or_default();
    let shared = profiles::shared_patches();
    let files: Vec<(&profiles::patches::PatchFile, bool)> = own.iter().map(|f| (*f, false)).chain(shared.iter().map(|f| (*f, true))).collect();
    let mut images: BTreeMap<String, Option<(translator::loader::nso::Nso, usize)>> = BTreeMap::new();
    for (f, sdk_wide) in files {
        for r in f.matches.iter().filter(|r| r.kind == "cuts") {
            let purpose = f.binding.iter().find(|b| b.what == r.name).map(|b| b.purpose.as_str()).unwrap_or("");
            if PLATFORM_PURPOSES.contains(&purpose) {
                continue;
            }
            let Some(bid) = builds.iter().find(|(m, _)| *m == r.module).map(|(_, b)| b.as_str()) else { continue };
            // a build with cuts of its own keeps them (and not the matched ones), as nxnative
            if !sdk_wide && own.iter().any(|g| g.cut.iter().any(|c| profiles::patches::build_matches(&c.build, bid))) {
                continue;
            }
            let entry = images.entry(r.module.clone()).or_insert_with(|| {
                let raw = exefs.get(&r.module)?;
                let nso = translator::loader::nso::Nso::parse(raw).ok()?;
                let end = nso.text.0 + nso.text.1;
                Some((nso, end))
            });
            let Some((nso, end)) = entry else { continue };
            let mut img = match Image::new(&nso.image, *end) {
                Ok(i) => i,
                Err(e) => {
                    said.push(format!("[RED] match {:?} on {}: {e}", r.name, r.module));
                    continue;
                }
            };
            let fns: Vec<(u64, Vec<String>, Vec<(u64, String, String, String)>)> = if r.functions.is_empty() {
                vec![(r.size.unwrap_or(0), r.fingerprints.clone(), r.cuts.iter().map(|c| (c.delta, c.original.clone(), c.replacement.clone(), c.reason.clone())).collect())]
            } else {
                r.functions.iter().map(|f| (f.size, f.fingerprints.clone(), f.cuts.iter().map(|c| (c.delta, c.original.clone(), c.replacement.clone(), c.reason.clone())).collect())).collect()
            };
            // every function of the rule matches once, at distinct places, or the rule applies nowhere
            let mut placed = vec![];
            let mut starts = vec![];
            let mut miss = None;
            for (i, (size, prints, cuts)) in fns.iter().enumerate() {
                match img.locate(*size as usize, prints).and_then(|s| img.cuts_at(s, cuts).map(|c| (s, c))) {
                    Ok((s, c)) if !starts.contains(&s) => {
                        starts.push(s);
                        placed.extend(c);
                    }
                    Ok(_) => miss = Some(format!("function {} matched one another function did", i + 1)),
                    Err(e) => miss = Some(format!("function {} of {}: {e}", i + 1, fns.len())),
                }
            }
            if let Some(m) = miss {
                if !sdk_wide {
                    said.push(format!("[RED] match {:?} on {}: not applied ({m})", r.name, r.module));
                }
                continue;
            }
            for (off, o, n, reason) in &placed {
                for k in 0..o.len() / 4 {
                    let w = |b: &[u8]| u32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap());
                    sites.push(format!("site match {} {:#x} {:08x} {:08x}  # {}", r.module, off + 4 * k as u64, w(o), w(n), reason.replace('#', "")));
                }
            }
            said.push(format!("{} on {} (function match, {} cut(s) at {})", r.name, r.module, placed.len(), starts.iter().map(|s| format!("{s:#x}")).collect::<Vec<_>>().join(", ")));
        }
    }
    (sites, said)
}

/// The patch table text with the matched sites added.
pub fn with_match_sites(text: Option<String>, sites: &[String]) -> Option<String> {
    if sites.is_empty() {
        return text;
    }
    let mut s = text.unwrap_or_else(|| String::from("# Written by nxrs from the game's profile: the patches applied at build time.\n"));
    s.push_str("set match NXRS_PATCH_MATCH 1\n");
    for l in sites {
        s.push_str(l);
        s.push('\n');
    }
    Some(s)
}

fn hex_u64(s: &str) -> Result<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| anyhow!("not a title id: {s}"))
}

/// `nxrs-build profile [<title id> [<main build id>]] [--package <input>]`
pub fn command(title: Option<&str>, build: Option<&str>, package: Option<&Path>, keys: Option<&Path>) -> Result<()> {
    let (tid, mods) = match package {
        Some(input) => {
            let opts = translator::package::OpenOptions { keys: keys.map(Path::to_path_buf), update: translator::package::UpdateChoice::Embedded };
            let g = translator::package::open_game(input, &opts)?;
            let mods = module_builds(&g.exefs.files);
            crate::out::line(&format!(
                "package {}: title {}, version {}",
                input.display(),
                g.exefs.title_id.map_or("unknown".into(), |t| format!("{t:016X}")),
                g.exefs.version.map_or("unknown".into(), |v| v.to_string())
            ));
            for (n, b) in &mods {
                crate::out::line(&format!("  {n:<8} {b}"));
            }
            (title.map(hex_u64).transpose()?.or(g.exefs.title_id), mods)
        }
        None => (title.map(hex_u64).transpose()?, build.map(|b| vec![("main".to_string(), b.to_ascii_lowercase())]).unwrap_or_default()),
    };
    let m = find(tid, &mods);
    let say = crate::out::line;
    say(&line(&m, tid, &mods));
    let Some(m) = m else { return Ok(()) };
    let p = m.profile;
    say(&format!("  match: {}", match m.how { How::Build => "the build is registered", How::Title => "by title only" }));
    say(&format!("  platform: {:?}, counter {} Hz", p.platform(), p.tick_hz()));
    say(&format!("  modules: {}", p.modules_of(m.build).join(" ")));
    let mm = &p.memory;
    say(&format!(
        "  memory: reserve {:#x} alias {:#x} stack {:#x} app_pool {:#x} used {:#x} sys_resource {:#x} stdalloc_cap {:#x} map_limit {}",
        mm.reserve, mm.alias, mm.stack, mm.app_pool, mm.used, mm.sys_resource, mm.stdalloc_cap, if mm.map_limit { "on" } else { "off" }
    ));
    let s = &p.settings;
    say(&format!(
        "  settings: fps_target {} fps_max {} fps_hold {} present_interval {} op_mode {} perf_mode {} core_preempt {} depth_mode {} pad_layout {}",
        s.fps_target, s.fps_max, s.fps_hold, s.present_interval, s.op_mode, s.perf_mode, s.core_preempt, s.depth_mode, s.pad_layout
    ));
    for b in &p.builds {
        let ms: Vec<String> = b.modules.iter().map(|(k, v)| format!("{k}={v}")).collect();
        say(&format!("  build {} {} {}", b.version, b.display, ms.join(" ")));
    }
    for f in p.patch_files() {
        say(&format!(
            "  patches ({}): {} cuts, {} stubs, {} match rules, {} observation gates",
            f.source,
            f.cut.len(),
            f.stub.len(),
            f.matches.len(),
            f.obs.len()
        ));
    }
    for (n, b) in &mods {
        for (src, e) in profiles::patches_for(b) {
            let what = match e {
                profiles::Entry::Cut(c) => format!("cut {n}+{:#x}{}: {}", c.offset, if c.apply.as_deref() == Some("build") { " (applied by nxrs)" } else { "" }, c.reason.lines().next().unwrap_or("")),
                profiles::Entry::Obs(o) => format!("observation gate {n}+{:#x}: {}", o.offset, o.reason),
                profiles::Entry::Stub(s) => format!("{} stub: {} words{}", s.kind, s.body.code.len(), if s.body.evaluated { String::new() } else { format!(" (needs {})", s.body.needs.as_deref().unwrap_or("?")) }),
                profiles::Entry::Capability(c) => format!("capabilities {}", c.names.join(" ")),
            };
            say(&format!("  for {n} {}: {what} [{src}]", &b[..16.min(b.len())]));
        }
    }
    Ok(())
}

#[cfg(test)]
mod match_tests {
    /// nxnative's MATCH cut for Pokémon Legends: Arceus (the capture demo's null weak reference, its crash fix) is
    /// found by fingerprint in the owner's 1.1.1 main and becomes sites of set `match` (run when the dump is here).
    #[test]
    fn arceus_match_cuts_become_sites() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/games/arceus/arceus_data/exefs");
        let Ok(main) = std::fs::read(dir.join("main")) else {
            eprintln!("no Arceus build here: skipped");
            return;
        };
        let exefs: std::collections::BTreeMap<String, Vec<u8>> = [("main".to_string(), main)].into();
        let builds = super::module_builds(&exefs);
        let prof = super::find(Some(0x0100_1F50_10DF_A000), &builds);
        assert!(prof.is_some());
        let (sites, said) = super::match_sites(&prof, &exefs, &builds);
        assert!(said.iter().any(|l| l.contains("capture demo") && l.contains("5 cut(s)")), "{said:?}");
        assert!(sites.iter().any(|s| s.starts_with("site match main 0x16e341c bf031af8 64020014")) || sites.iter().any(|s| s.contains("0x16e341c")), "{sites:?}");
        let t = super::with_match_sites(None, &sites).unwrap();
        assert!(t.contains("set match NXRS_PATCH_MATCH 1"));
    }
}

#[cfg(test)]
mod match_tests_oot {
    /// Ocarina of Time's MATCH cuts on the owner's build, when the dump is here: every rule either applies or says why.
    #[test]
    fn oot_match_cuts() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/games/oot/oot_data/exefs");
        let Ok(main) = std::fs::read(dir.join("main")) else {
            eprintln!("no OoT build here: skipped");
            return;
        };
        let exefs: std::collections::BTreeMap<String, Vec<u8>> = [("main".to_string(), main)].into();
        let builds = super::module_builds(&exefs);
        let prof = super::find(Some(0x0400_4860_24DE_C000), &builds);
        let (sites, said) = super::match_sites(&prof, &exefs, &builds);
        for l in &said {
            eprintln!("{l}");
        }
        eprintln!("{} sites", sites.len());
    }
}

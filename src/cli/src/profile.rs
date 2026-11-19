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

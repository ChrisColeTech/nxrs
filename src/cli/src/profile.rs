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
/// What a build's frame-step and stub patches give the game: the table lines and the facts the host's frame step
/// reads (`frame_step.txt` in the data folder).
#[derive(Debug, Default)]
pub struct Stubs {
    pub lines: Vec<String>,
    pub said: Vec<String>,
    /// a frame-step set placed (the host then writes its state page and paces the game by it)
    pub frame_step: bool,
    pub step_max: u32,
    pub render_scale: u32,
    pub capabilities: Vec<String>,
}

impl Stubs {
    /// `frame_step.txt`: one `key value` a line.
    pub fn facts(&self) -> String {
        format!(
            "# Written by nxrs: the frame step of this build's patches (the host's state page writer reads it).\nframe_step {}\nstep_max {}\nrender_scale {}\ncapabilities {}\n",
            self.frame_step as u8,
            self.step_max,
            self.render_scale,
            self.capabilities.join(",")
        )
    }
}

/// nxnative's FRAME_STEP / STUBS (`[[stub]]` by build) and MATCH_FRAME_STEP / MATCH_STUBS (`[[match]]` kind
/// frame_step / stubs, placed by function fingerprint on builds without an entry of their own) as patch-table lines:
/// each module's stubs go to its patch area (`area` / `areloc`), the puts to sites that branch there (set `fstep`,
/// `NXRS_PATCH_FSTEP=0` off; set `stubs`, `NXRS_PATCH_STUBS=0` off). Bodies not evaluated at export (Python that reads
/// the module) and rules whose functions or asserted sites are not found stay out, said in red.
pub fn stubs(prof: &Option<Match>, exefs: &BTreeMap<String, Vec<u8>>, builds: &[(String, String)]) -> Stubs {
    use translator::aot::nso_match::Image;
    let mut st = Stubs::default();
    let own: Vec<&profiles::patches::PatchFile> = prof.as_ref().map(|m| m.profile.patch_files().collect()).unwrap_or_default();
    let shared = profiles::shared_patches();
    let files: Vec<&profiles::patches::PatchFile> = own.iter().copied().chain(shared.iter().copied()).collect();
    let mut nsos: BTreeMap<String, Option<translator::loader::nso::Nso>> = BTreeMap::new();
    // {module: area words so far}
    let mut area_len: BTreeMap<String, usize> = BTreeMap::new();
    let (mut fstep_set, mut stubs_set) = (false, false);
    for (module, bid) in builds {
        let nso = nsos.entry(module.clone()).or_insert_with(|| exefs.get(module).and_then(|r| translator::loader::nso::Nso::parse(r).ok()));
        let Some(nso) = nso.as_ref() else { continue };
        let end = nso.text.0 + nso.text.1;
        #[allow(clippy::type_complexity)]
        let mut bodies: Vec<(String, &str, &profiles::patches::StubBody, BTreeMap<String, u64>, String, Option<String>)> = vec![];
        for f in &files {
            for s in f.stub.iter().filter(|s| profiles::patches::build_matches(&s.build, bid) && module == "main") {
                bodies.push((format!("{} {}", s.kind, &s.build[..8.min(s.build.len())]), s.kind.as_str(), &s.body, BTreeMap::new(), s.build.clone(), None));
            }
        }
        let has_own_fstep = bodies.iter().any(|b| b.1 == "frame_step");
        let mut img: Option<Image> = None;
        for f in &files {
            for r in f.matches.iter().filter(|r| (r.kind == "frame_step" || r.kind == "stubs") && r.module == *module) {
                let Some(body) = &r.stub else { continue };
                if r.kind == "frame_step" && has_own_fstep {
                    continue;
                }
                if img.is_none() {
                    match Image::new(&nso.image, end) {
                        Ok(i) => img = Some(i),
                        Err(e) => {
                            st.said.push(format!("[RED] {module}: {e}"));
                            break;
                        }
                    }
                }
                let i = img.as_mut().unwrap();
                let mut at = BTreeMap::new();
                // an anchored rule (the caller of an import) is found by its generator
                let anchored = r.fingerprints.is_empty() && r.calls.is_some();
                let found = if anchored { Ok(None) } else { i.locate(r.size.unwrap_or(0) as usize, &r.fingerprints).map(|s| at.insert("base".to_string(), s as u64)) };
                if found.is_err() {
                    continue; // not this build's function: quietly, as the cuts
                }
                let mut miss = None;
                for o in &r.others {
                    match i.locate(o.size as usize, &o.fingerprints) {
                        Ok(s) => {
                            at.insert(format!("other:{}", o.label), s as u64);
                        }
                        Err(e) => miss = Some(format!("{}: {e}", o.label)),
                    }
                }
                if let Some(m) = miss {
                    st.said.push(format!("[RED] {} {:?} on {module}: not applied ({m})", r.kind, r.name));
                    continue;
                }
                bodies.push((format!("{} {:?}", r.kind, r.name), if r.kind == "stubs" { "stub" } else { "frame_step" }, body, at, r.name.clone(), r.calls.clone()));
            }
        }
        for (what, kind, b, at, key, calls) in bodies {
            // a body that reads the module: its generator, run against this main (crate::stubgen)
            let generated;
            let b = if b.evaluated {
                b
            } else {
                let others: BTreeMap<String, u64> = at.iter().filter_map(|(k, v)| k.strip_prefix("other:").map(|l| (l.to_string(), *v))).collect();
                match crate::stubgen::generate(&key, nso, at.get("base").copied().unwrap_or(0), &others, calls.as_deref()) {
                    Some(Ok(g)) => {
                        generated = g;
                        &generated
                    }
                    Some(Err(e)) => {
                        st.said.push(format!("[RED] {what} on {module}: not applied ({e})"));
                        continue;
                    }
                    None => {
                        st.said.push(format!("[RED] {what} on {module}: not applied (its body reads the module: {})", b.needs.as_deref().unwrap_or("?")));
                        continue;
                    }
                }
            };
            let place = |base: &str| -> Option<u64> {
                if base == "module" {
                    Some(0)
                } else {
                    at.get(base).copied()
                }
            };
            let word_at = |off: u64| -> Option<u32> {
                let o = off as usize;
                (o >= nso.text.0 && o + 4 <= end).then(|| u32::from_le_bytes(nso.image[o..o + 4].try_into().unwrap()))
            };
            // the asserted sites: the module still has the words the stub was written against
            let bad = b.sites.iter().find_map(|s| {
                let off = place(&s.base)?.checked_add_signed(s.offset)?;
                let want = profiles::patches::words_of(&s.original)?;
                (0..want.len()).any(|k| word_at(off + 4 * k as u64) != Some(want[k])).then(|| format!("{}+{:#x}", s.base, s.offset)).or(None)
            });
            if let Some(b) = bad {
                st.said.push(format!("[RED] {what} on {module}: not applied (the site {b} differs from the recorded words)"));
                continue;
            }
            if b.sites.iter().any(|s| place(&s.base).is_none()) {
                st.said.push(format!("[RED] {what} on {module}: not applied (a site's function is not located)"));
                continue;
            }
            let start = *area_len.get(module).unwrap_or(&0);
            let reloc = |r: &profiles::patches::Reloc, word: Option<u32>| -> Option<String> {
                let (target, addend) = match r.target.as_str() {
                    "state" => ("state", r.addend),
                    "area" => ("area", r.addend + 4 * start as i64),
                    "module" => ("module", r.addend),
                    other => ("module", place(other)? as i64 + r.addend),
                };
                if !matches!(r.op.as_str(), "adrp" | "b" | "bl") {
                    return None;
                }
                let add = format!("{}{:#x}", if addend < 0 { "-" } else { "" }, addend.unsigned_abs());
                Some(match word {
                    Some(w) => format!("@{}:{target}:{add}:{w:08x}", r.op),
                    None => format!("{} {target} {add}", r.op),
                })
            };
            let set = if kind == "frame_step" { "fstep" } else { "stubs" };
            let mut lines = vec![];
            let mut ok = true;
            for chunk in b.code.chunks(16) {
                lines.push(format!("area {set} {module} {}", chunk.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>().join(" ")));
            }
            for r in &b.relocs {
                match reloc(r, None) {
                    Some(t) => lines.push(format!("areloc {module} {} {t}", start + r.index)),
                    None => ok = false,
                }
            }
            for p in &b.puts {
                let Some(off) = place(&p.base).and_then(|o| o.checked_add_signed(p.offset)) else {
                    ok = false;
                    break;
                };
                for (k, w) in p.words.iter().enumerate() {
                    let a = off + 4 * k as u64;
                    let Some(orig) = word_at(a) else {
                        ok = false;
                        break;
                    };
                    let repl = match p.relocs.iter().find(|r| r.index == k) {
                        Some(r) => match reloc(r, Some(*w)) {
                            Some(t) => t,
                            None => {
                                ok = false;
                                break;
                            }
                        },
                        None => format!("{w:08x}"),
                    };
                    lines.push(format!("site {set} {module} {a:#x} {orig:08x} {repl}  # {}", p.reason.lines().next().unwrap_or("").replace('#', "")));
                }
            }
            if !ok {
                st.said.push(format!("[RED] {what} on {module}: not applied (a relocation or put this table cannot place)"));
                continue;
            }
            st.lines.extend(lines);
            area_len.insert(module.clone(), start + b.code.len());
            if kind == "frame_step" {
                fstep_set = true;
                st.frame_step = true;
                st.step_max = st.step_max.max(b.step_max.unwrap_or(0));
                st.render_scale = st.render_scale.max(b.render_scale.unwrap_or(0));
                st.capabilities.extend(b.capabilities.clone().unwrap_or_default());
            } else {
                stubs_set = true;
            }
            st.said.push(format!("{what} on {module} ({} words, {} sites)", b.code.len(), b.puts.iter().map(|p| p.words.len()).sum::<usize>()));
        }
    }
    let mut head = vec![];
    if fstep_set {
        head.push("set fstep NXRS_PATCH_FSTEP 1".to_string());
    }
    if stubs_set {
        head.push("set stubs NXRS_PATCH_STUBS 1".to_string());
    }
    st.lines.splice(0..0, head);
    st
}

/// The completer of Game Freak's Trinity per-frame rendezvous (Pokémon Scarlet / Violet, Legends Z-A): it sets the
/// release flag with a plain `strb`, then reads the state word with `ldar` and returns without a wake when it is 2. A
/// load-acquire does not order an earlier plain store (x86 also moves a store after a later load), so the flag can
/// stay unseen while the main thread resets the state, reads the flag as 0 and parks for good. The site gets a
/// `dmb ish` (a SEQ_CST fence) after the store (set `trinity`, `NXRS_PATCH_TRINITY=0` off). The shape is nxnative's
/// (`patch_nso.py` trinity_completer_sites): two matches in one module place none. Returns (lines, said).
pub fn trinity_sites(exefs: &BTreeMap<String, Vec<u8>>, builds: &[(String, String)]) -> (Vec<String>, Vec<String>) {
    let (mut lines, mut said) = (vec![], vec![]);
    for (module, _) in builds {
        let Some(nso) = exefs.get(module).and_then(|r| translator::loader::nso::Nso::parse(r).ok()) else { continue };
        let found = trinity_in(&nso.image, nso.text.0, nso.text.0 + nso.text.1);
        match found.as_slice() {
            [] => {}
            [f] => {
                let strb = u32::from_le_bytes(nso.image[f + 20..f + 24].try_into().unwrap());
                lines.push(format!("site trinity {module} {:#x} {strb:08x} {strb:08x} d5033bbf  # Trinity rendezvous completer: a fence after the release flag's store", f + 20));
                said.push(format!("Trinity rendezvous fence on {module} at {f:#x}"));
            }
            many => said.push(format!(
                "[RED] {module}: {} functions have the shape of Trinity's rendezvous completer, so none gets its fence: {}",
                many.len(),
                many.iter().map(|o| format!("{o:#x}")).collect::<Vec<_>>().join(", ")
            )),
        }
    }
    if !lines.is_empty() {
        lines.insert(0, "set trinity NXRS_PATCH_TRINITY 1".into());
    }
    (lines, said)
}

/// Offsets in [s, e) of functions shaped as Trinity's rendezvous completer.
fn trinity_in(img: &[u8], s: usize, e: usize) -> Vec<usize> {
    let w = |o: usize| u32::from_le_bytes(img[o..o + 4].try_into().unwrap());
    let mut out = vec![];
    let mut o = s;
    while o + 12 <= e.min(img.len()) {
        // ldar w8,[x9]; cmp w8,#2
        if w(o) == 0x88DF_FD28 && w(o + 4) == 0x7100_091F && o >= s + 40 {
            let f = o - 40;
            let v: Vec<u32> = (0..13).map(|i| w(f + 4 * i)).collect();
            if v[..3] == [0xA9BE_7BFD, 0xA901_4FF4, 0x9100_03FD]
                && v[3] & 0xFFC0_03FF == 0xF940_0008
                && v[4] == 0x5280_0029
                && v[5] & 0xFFC0_03FF == 0x3900_0109
                && v[6] & 0xFFC0_03FF == 0x3940_0109
                && (v[6] >> 10 & 0xFFF) == (v[5] >> 10 & 0xFFF) + 1
                && v[7] & 0xFF00_001F == 0x3400_0009
                && v[8] & 0xFFC0_03FF == 0xF940_0114
                && v[9] & 0xFFC0_03FF == 0x9100_0289
                && v[12] & 0xFF00_001F == 0x5400_0001
            {
                out.push(f);
            }
        }
        o += 4;
    }
    out
}

/// The patch table text with more lines (sets and all) added.
pub fn with_lines(text: Option<String>, lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return text;
    }
    let mut s = text.unwrap_or_else(|| String::from("# Written by nxrs from the game's profile: the patches applied at build time.\n"));
    for l in lines {
        s.push_str(l);
        s.push('\n');
    }
    Some(s)
}

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

#[cfg(test)]
mod stub_tests {
    /// The frame-step rules of Ocarina of Time (frame ratio + dynamic resolution) and Mario Kart World (frame
    /// interval) on the owner's builds, when the dumps are here: placed by fingerprint, and the table the translator
    /// reads takes them (sites match the module, the stubs module lands after main's patch page, relocations reach).
    #[test]
    fn frame_step_on_owner_builds() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/games");
        for (game, title) in [("oot", 0x0400_4860_24DE_C000u64), ("mkw", 0x0400_C3F0_0006_E000)] {
            let Ok(main) = std::fs::read(root.join(game).join(format!("{game}_data/exefs/main"))) else {
                eprintln!("no {game} build here: skipped");
                continue;
            };
            let exefs: std::collections::BTreeMap<String, Vec<u8>> = [("main".to_string(), main)].into();
            let builds = super::module_builds(&exefs);
            let prof = super::find(Some(title), &builds);
            let st = super::stubs(&prof, &exefs, &builds);
            for l in &st.said {
                eprintln!("{game}: {l}");
            }
            assert!(st.frame_step, "{game}: no frame step placed");
            let text = super::with_lines(None, &st.lines).unwrap();
            let nso = translator::loader::nso::Nso::parse(&exefs["main"]).unwrap();
            let lay = vec![translator::loader::ModLayout { name: "main".into(), base: 0x80_0000_0000, size: nso.size as u64 }];
            let t = translator::aot::patches::Table::parse(&text, game, &lay).unwrap();
            let m = translator::loader::Module::new("main", nso, 0x80_0000_0000);
            t.check(&m, "main").unwrap();
            let (name, _, _, code, words) = t.stubs_module("main", m.base).unwrap().unwrap();
            assert_eq!(name, "main_stubs");
            let sites = t.for_module("main", m.base).unwrap();
            for (a, (_, ws)) in &sites {
                if let Some(to) = translator::aot::patches::branch_target(ws[0], *a) {
                    assert!((code..code + 4 * words.len() as u64).contains(&to), "{game}: site {a:#x} branches to {to:#x}, outside the area");
                }
            }
            eprintln!("{game}: {} area words, {} sites, step_max {}", words.len(), sites.len(), st.step_max);
        }
    }
}

#[cfg(test)]
mod trinity_tests {
    /// Violet's main (when the dump is here) has exactly one rendezvous completer, and the table takes its fence.
    #[test]
    fn violet_rendezvous_fence() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/games/violet/violet_data/exefs/main");
        let Ok(main) = std::fs::read(&p) else {
            eprintln!("no Violet build here: skipped");
            return;
        };
        let exefs: std::collections::BTreeMap<String, Vec<u8>> = [("main".to_string(), main)].into();
        let builds = super::module_builds(&exefs);
        let (lines, said) = super::trinity_sites(&exefs, &builds);
        eprintln!("{said:?}");
        assert_eq!(lines.len(), 2, "{said:?}");
        let nso = translator::loader::nso::Nso::parse(&exefs["main"]).unwrap();
        let lay = vec![translator::loader::ModLayout { name: "main".into(), base: 0x80_0000_0000, size: nso.size as u64 }];
        let t = translator::aot::patches::Table::parse(&lines.join("\n"), "violet", &lay).unwrap();
        t.check(&translator::loader::Module::new("main", nso, 0x80_0000_0000), "main").unwrap();
    }
}

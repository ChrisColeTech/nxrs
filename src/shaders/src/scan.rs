//! A whole game's shaders: every code module of its ExeFS and every file of its RomFS (the update applied), read in
//! place through the translator's package API, in parallel, one copy kept per key.

use crate::formats::{self, archive, engines, gamefreak, Ctx, Scanned};
use crate::nvn::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use translator::package::RomFs;

/// A file larger than this is not read whole (packs read by range are not affected).
pub const MAX_WHOLE: u64 = 3 << 30;
/// Trinity packs are read in groups of about this many bytes.
const PACK_GROUP: u64 = 256 << 20;

/// One shader, one copy per key.
#[derive(Debug, Clone)]
pub struct Shader {
    pub key: String,
    pub stage: Stage,
    pub ctl: Vec<u8>,
    pub code: Vec<u8>,
    /// Where it first appeared.
    pub source: String,
    /// Copies found in all (the same key in several files).
    pub copies: usize,
}

/// A vertex (or mesh) shader and the pixel shader it is drawn with, from the same program variation; `prog` marks
/// agl BAHS program variations (supplemental pairs).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pair {
    pub first: String,
    pub pixel: String,
    pub prog: bool,
}

#[derive(Default)]
pub struct ScanReport {
    pub shaders: BTreeMap<String, Shader>,
    pub pairs: BTreeSet<Pair>,
    /// Parts of files that could not be read (their shaders are left out).
    pub problems: Vec<String>,
    /// Same key, different bytes (should never happen).
    pub conflicts: Vec<String>,
    pub files: usize,
    pub bytes_read: u64,
    pub seconds: f64,
    /// Readers that found shaders, by tag (BNSH, BAHS, GLSLC, ...): copies found.
    pub by_reader: BTreeMap<String, usize>,
    pub zstd_dicts: usize,
    pub oodle_streams: usize,
}

/// A work item; the `usize` is the RomFS it reads (0 the game's, then the add-ons').
enum Item {
    Code(String, Arc<Vec<u8>>),
    File(usize, String, u64),
    Packs(usize, String, Vec<(u64, u64)>),
    Cpk(usize, String, Vec<(String, u64, u64, u64)>),
    /// A Heaps PAK's shader cache: (the PAK, the member's path, offset, length).
    Heaps(usize, String, String, u64, u64),
}

impl Item {
    fn weight(&self) -> u64 {
        match self {
            Item::Code(_, d) => d.len() as u64,
            Item::File(_, _, n) => *n,
            Item::Packs(_, _, p) => p.iter().map(|x| x.1).sum(),
            Item::Cpk(_, _, e) => e.iter().map(|x| x.2).sum(),
            Item::Heaps(_, _, _, _, n) => *n,
        }
    }
}

fn reader_tag(label: &str) -> String {
    let t = label.rsplit('#').find(|p| p.chars().next().is_some_and(|c| c.is_ascii_uppercase())).unwrap_or("?");
    t.split(['@', ':']).next().unwrap_or("?").to_string()
}

/// Every shader of a game: its code modules (`exefs`: name → NSO bytes) and its RomFS.
pub fn scan(exefs: &BTreeMap<String, Vec<u8>>, romfs: &RomFs, jobs: usize, say: &(dyn Fn(&str) + Sync)) -> ScanReport {
    scan_with_addons(exefs, romfs, &[], jobs, say)
}

/// A copy's place in the choice among copies of one key: the game's own before an add-on's, then by path, so the
/// output does not depend on the threads.
fn rank(label: &str) -> (bool, &str) {
    (label.starts_with("/aoc/"), label)
}

/// The same, with the RomFS of installed add-on content (labels `/aoc/<title id>/...`).
pub fn scan_with_addons(exefs: &BTreeMap<String, Vec<u8>>, romfs: &RomFs, addons: &[(u64, &RomFs)], jobs: usize, say: &(dyn Fn(&str) + Sync)) -> ScanReport {
    let mut roms: Vec<(String, &RomFs)> = vec![(String::new(), romfs)];
    roms.extend(addons.iter().map(|(t, r)| (format!("aoc/{t:016x}/"), *r)));
    let t0 = Instant::now();
    let mut rep = ScanReport::default();
    let mut ctx = Ctx::default();
    if romfs.find("Pack/ZsDic.pack.zs").is_some_and(|n| n.is_file()) {
        match romfs.read_file("Pack/ZsDic.pack.zs").map_err(|e| e.to_string()).and_then(|d| archive::ZstdDicts::from_pack(&d, "/Pack/ZsDic.pack.zs")) {
            Ok(d) => ctx.dicts = d,
            Err(e) => rep.problems.push(format!("/Pack/ZsDic.pack.zs: {e}")),
        }
    }
    rep.zstd_dicts = ctx.dicts.len();

    // the work: code modules, then the RomFS (packs and archives split by their own index)
    let mut items: Vec<Item> = vec![];
    for (name, nso) in exefs {
        if nso.get(..4) != Some(b"NSO0") {
            continue;
        }
        match formats::nso::image(nso, &format!("exefs/{name}")) {
            Ok(img) => items.push(Item::Code(format!("exefs/{name}"), Arc::new(img))),
            Err(e) => rep.problems.push(e),
        }
    }
    let mut trinity: Vec<(usize, String, u64)> = vec![];
    for (ri, (_, romfs)) in roms.iter().enumerate() {
        let romfs = *romfs;
        let files = romfs.walk();
        rep.files += files.len();
        for (path, size) in files {
        let head = romfs.read(&path, 0, 16).unwrap_or_default();
        let plan = (|| -> Result<Option<Item>, String> {
            if head.starts_with(gamefreak::ONEPACK) && size >= 16 {
                trinity.push((ri, path.clone(), size));
                return Ok(None);
            }
            if head.starts_with(b"CPK ") {
                let read = |o: u64, n: u64| romfs.read(&path, o, n).map_err(|e| e.to_string());
                let ents = archive::cpk_files(&read, size)?;
                let mut cur = vec![];
                let mut n = 0;
                for e in ents {
                    n += e.2;
                    cur.push(e);
                    if n >= PACK_GROUP {
                        items.push(Item::Cpk(ri, path.clone(), std::mem::take(&mut cur)));
                        n = 0;
                    }
                }
                if !cur.is_empty() {
                    items.push(Item::Cpk(ri, path.clone(), cur));
                }
                return Ok(None);
            }
            if head.starts_with(b"PAK") && head.len() >= 12 {
                let hs = i32_at(&head, 4).unwrap_or(0).max(0) as u64;
                if hs > 12 && hs <= 1 << 28 {
                    let h = romfs.read(&path, 0, hs).map_err(|e| e.to_string())?;
                    if let Some(members) = engines::heaps_pak_files(&h) {
                        for (nm, off, n) in members {
                            if nm.ends_with(&format!("/{}", engines::HEAPS_CACHE)) {
                                items.push(Item::Heaps(ri, path.clone(), nm, off, n));
                            }
                        }
                        return Ok(None);
                    }
                }
            }
            Ok(Some(Item::File(ri, path.clone(), size)))
        })();
        match plan {
            Ok(Some(it)) => items.push(it),
            Ok(None) => {}
            Err(e) => {
                rep.problems.push(format!("/{}{path}: {e}; scanned as one file", roms[ri].0));
                items.push(Item::File(ri, path.clone(), size));
            }
        }
        }
    }
    for (ri, path, size) in trinity {
        if let Err(e) = trinity_items(ri, &path, size, roms[ri].1, &mut items) {
            rep.problems.push(format!("/{}{path}: {e}; not scanned", roms[ri].0));
        }
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.weight()));
    let total = items.len();
    say(&format!("shaders: scanning {} files ({} work items, {} jobs)", rep.files, total, jobs));

    let shaders: Mutex<HashMap<String, Shader>> = Mutex::new(HashMap::new());
    let pairs: Mutex<BTreeSet<Pair>> = Mutex::new(BTreeSet::new());
    let problems: Mutex<Vec<String>> = Mutex::new(vec![]);
    let conflicts: Mutex<Vec<String>> = Mutex::new(vec![]);
    let readers: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    let oodle = AtomicUsize::new(0);
    let last = Mutex::new(Instant::now());
    let items = &items;
    let ctx = &ctx;
    let roms = &roms;
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                let mut out = Scanned::default();
                run_item(ctx, roms, item, &mut out, &bytes, &oodle);
                // pairs of this item's program variations
                let mut groups: HashMap<&str, HashMap<Stage, String>> = HashMap::new();
                for f in &out.found {
                    if let Some(g) = &f.group {
                        groups.entry(g.as_str()).or_default().insert(f.stage, f.key());
                    }
                }
                {
                    let mut p = pairs.lock().unwrap();
                    for (g, m) in &groups {
                        if let Some(ps) = m.get(&Stage::Ps) {
                            for st in [Stage::Vs, Stage::Ms] {
                                if let Some(v) = m.get(&st) {
                                    p.insert(Pair { first: v.clone(), pixel: ps.clone(), prog: g.contains("#BAHS#prog") });
                                }
                            }
                        }
                    }
                }
                {
                    let mut r = readers.lock().unwrap();
                    for f in &out.found {
                        *r.entry(reader_tag(&f.label)).or_default() += 1;
                    }
                }
                {
                    let mut m = shaders.lock().unwrap();
                    for f in out.found {
                        let key = f.key();
                        match m.get_mut(&key) {
                            Some(old) => {
                                old.copies += 1;
                                if !same_shader((&old.ctl, &old.code), (&f.ctl, &f.code)) {
                                    conflicts.lock().unwrap().push(format!("key {key} is the same but the control section or code area differs ({} / {})", f.label, old.source));
                                }
                                // the first copy in path order is kept, so the output does not depend on the threads
                                if rank(&f.label) < rank(&old.source) {
                                    old.source = f.label;
                                    old.ctl = f.ctl;
                                    old.code = f.code;
                                    old.stage = f.stage;
                                }
                            }
                            None => {
                                m.insert(key.clone(), Shader { key, stage: f.stage, ctl: f.ctl, code: f.code, source: f.label, copies: 1 });
                            }
                        }
                    }
                }
                problems.lock().unwrap().extend(out.problems);
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                let mut l = last.lock().unwrap();
                if l.elapsed().as_secs_f64() >= 2.0 || n == total {
                    *l = Instant::now();
                    say(&format!("shaders: scanned {n} of {total} work items, {} shaders, {:.0} MB read", shaders.lock().unwrap().len(), bytes.load(Ordering::Relaxed) as f64 / 1e6));
                }
            });
        }
    });
    rep.shaders = shaders.into_inner().unwrap().into_iter().collect();
    rep.pairs = pairs.into_inner().unwrap();
    let mut pr = problems.into_inner().unwrap();
    pr.sort();
    rep.problems.extend(pr);
    rep.conflicts = conflicts.into_inner().unwrap();
    rep.by_reader = readers.into_inner().unwrap();
    rep.bytes_read = bytes.load(Ordering::Relaxed);
    rep.oodle_streams = oodle.load(Ordering::Relaxed);
    rep.seconds = t0.elapsed().as_secs_f64();
    rep
}

/// A Trinity file system's packs, in work items of about PACK_GROUP bytes. A pack runs to the next one (the last to
/// the root table).
fn trinity_items(ri: usize, path: &str, size: u64, romfs: &RomFs, items: &mut Vec<Item>) -> Result<(), String> {
    let head = romfs.read(path, 0, 16).map_err(|e| e.to_string())?;
    let root = u64_at(&head, 8).unwrap_or(size).min(size);
    let tail = romfs.read(path, root, size - root).map_err(|e| e.to_string())?;
    let offs = gamefreak::trpfs_packs(&head, &tail)?;
    let mut cur = vec![];
    let mut n = 0;
    for (i, &o) in offs.iter().enumerate() {
        let end = offs.get(i + 1).copied().unwrap_or(root).min(size);
        if o >= end {
            continue;
        }
        cur.push((o, end - o));
        n += end - o;
        if n >= PACK_GROUP {
            items.push(Item::Packs(ri, path.to_string(), std::mem::take(&mut cur)));
            n = 0;
        }
    }
    if !cur.is_empty() {
        items.push(Item::Packs(ri, path.to_string(), cur));
    }
    Ok(())
}

fn run_item(ctx: &Ctx, roms: &[(String, &RomFs)], item: &Item, out: &mut Scanned, bytes: &AtomicU64, oodle: &AtomicUsize) {
    match item {
        Item::Code(label, image) => formats::scan_code(ctx, image, label, out),
        Item::File(ri, path, size) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let label = format!("/{pre}{path}");
            if *size > MAX_WHOLE {
                out.problems.push(format!("{label}: {size} bytes, too large to read whole (not scanned)"));
                return;
            }
            match romfs.read_file(path) {
                Ok(d) => {
                    bytes.fetch_add(d.len() as u64, Ordering::Relaxed);
                    formats::scan_file(ctx, &d, &label, out);
                }
                Err(e) => out.problems.push(format!("{label}: {e}")),
            }
        }
        Item::Packs(ri, path, packs) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for &(off, len) in packs {
                let label = format!("/{pre}{path}#{off:#x}");
                let pack = match romfs.read(path, off, len) {
                    Ok(p) => p,
                    Err(e) => {
                        out.problems.push(format!("{label}: {e}"));
                        continue;
                    }
                };
                bytes.fetch_add(pack.len() as u64, Ordering::Relaxed);
                let r = gamefreak::trinity_pack(&pack, &label, &mut |h, data| {
                    oodle.fetch_add(1, Ordering::Relaxed);
                    formats::scan_file(ctx, &data, &format!("{label}#{h:016x}"), out);
                    Ok(())
                });
                if let Err(e) = r {
                    out.problems.push(e);
                }
            }
        }
        Item::Cpk(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for (nm, off, csz, usz) in ents {
                let label = format!("/{pre}{path}/{nm}");
                let d = romfs.read(path, *off, *csz).map_err(|e| e.to_string()).and_then(|raw| archive::cpk_entry(raw, *usz, &label));
                match d {
                    Ok(d) => {
                        bytes.fetch_add(*csz, Ordering::Relaxed);
                        formats::scan_blob(ctx, &d, &label, out, 0);
                    }
                    Err(e) => out.problems.push(format!("{label}: {e}; not scanned")),
                }
            }
        }
        Item::Heaps(ri, pak, member, off, n) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let label = format!("/{pre}{pak}{member}");
            match romfs.read(pak, *off, *n) {
                Ok(d) => match engines::heaps_shaders(&d, &label) {
                    Ok(f) => out.found.extend(f),
                    Err(e) => out.problems.push(e),
                },
                Err(e) => out.problems.push(format!("{label}: {e}")),
            }
        }
    }
}

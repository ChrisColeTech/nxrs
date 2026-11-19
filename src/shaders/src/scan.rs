//! A whole game's shaders: every code module of its ExeFS and every file of its RomFS (the update applied), read in
//! place through the translator's package API, in parallel, one copy kept per key.

use crate::formats::{self, archive, capcom, engines, fromsoft, gamefreak, koei, smash, unreal, Ctx, Scanned};
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
    /// Files of an Unreal pak (its shader code archives, global shader caches, cooked packages).
    Pak(usize, String, Vec<unreal::PakEntry>, Option<[u8; 32]>),
    /// Entries of a FromSoftware dvdbnd archive (hash, offset, size).
    Bdt(usize, String, Vec<(u32, u64, u64)>),
    /// Files of Super Smash Bros. Ultimate's data.arc.
    Smash(usize, String, Vec<(u64, u64, u64, u64)>),
    /// A KTGL RDB file (IDRK entries), read entry by entry.
    Rdb(usize, String, u64),
    /// Entries of a LINKDATA archive (offset, length, stored length, compressed).
    Linkdata(usize, String, Vec<(usize, (u64, u64, u64, u64))>),
    /// Entries of an RE Engine PAK, with its block table.
    Kpka(usize, String, Vec<capcom::KpkaEntry>, Arc<Option<(u64, Vec<(u64, u64)>)>>),
    /// Chunks of an IoStore container (the .utoc's path): its shader libraries (true) or its cooked packages.
    Io(usize, String, Arc<unreal::Toc>, Vec<usize>, bool, Option<[u8; 32]>),
}

impl Item {
    fn weight(&self) -> u64 {
        match self {
            Item::Code(_, d) => d.len() as u64,
            Item::File(_, _, n) => *n,
            Item::Packs(_, _, p) => p.iter().map(|x| x.1).sum(),
            Item::Cpk(_, _, e) => e.iter().map(|x| x.2).sum(),
            Item::Heaps(_, _, _, _, n) => *n,
            Item::Pak(_, _, e, _) => e.iter().map(|x| x.size).sum(),
            Item::Kpka(_, _, e, _) => e.iter().map(|x| x.stored).sum(),
            Item::Rdb(_, _, n) => *n,
            Item::Smash(_, _, e) => e.iter().map(|x| x.2).sum(),
            Item::Bdt(_, _, e) => e.iter().map(|x| x.2).sum(),
            Item::Linkdata(_, _, e) => e.iter().map(|x| x.1 .2).sum(),
            Item::Io(_, _, t, c, _, _) => c.iter().map(|&i| t.ranges[i].1).sum(),
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
    let mut pak_problems: Vec<String> = vec![];
    // Unreal: main's image and its read-only segment's start (AES keys are searched there), the keys found, the
    // IoStore containers, whether any pak holds shader code archives
    let main_img: std::cell::OnceCell<Option<(Vec<u8>, usize)>> = std::cell::OnceCell::new();
    let main = || {
        main_img
            .get_or_init(|| {
                let nso = exefs.get("main")?;
                let img = formats::nso::image(nso, "exefs/main").ok()?;
                Some((img, u32_at(nso, 0x24)? as usize))
            })
            .as_ref()
    };
    let mut ue_keys: Vec<[u8; 32]> = vec![];
    let mut pak_keys: HashMap<(usize, String), [u8; 32]> = HashMap::new();
    let mut tocs: Vec<(usize, String)> = vec![];
    let mut ue_archives = false;
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
            if let Some(idx) = path.strip_suffix(".bdt").map(|p| format!("{p}.bhd5")).filter(|i| romfs.find(i).is_some_and(|n| n.is_file())) {
                let ents = fromsoft::bhd5_entries(&romfs.read_file(&idx).map_err(|e| e.to_string())?, &idx)?;
                let mut cur = vec![];
                let mut n = 0;
                for e in ents {
                    n += e.2;
                    cur.push(e);
                    if n >= PACK_GROUP {
                        items.push(Item::Bdt(ri, path.clone(), std::mem::take(&mut cur)));
                        n = 0;
                    }
                }
                if !cur.is_empty() {
                    items.push(Item::Bdt(ri, path.clone(), cur));
                }
                return Ok(None);
            }
            if head.starts_with(&smash::MAGIC) {
                let read = |o: u64, n: u64| romfs.read(&path, o, n).map_err(|e| e.to_string());
                let ents = smash::files(&read, &path)?;
                let mut cur = vec![];
                let mut n = 0;
                for e in ents {
                    n += e.2;
                    cur.push(e);
                    if n >= PACK_GROUP {
                        items.push(Item::Smash(ri, path.clone(), std::mem::take(&mut cur)));
                        n = 0;
                    }
                }
                if !cur.is_empty() {
                    items.push(Item::Smash(ri, path.clone(), cur));
                }
                return Ok(None);
            }
            if head.starts_with(koei::IDRK) || head.starts_with(koei::PDRK) {
                return Ok(Some(Item::Rdb(ri, path.clone(), size)));
            }
            if let Some(idx) = koei::linkdata_index(&path).filter(|i| romfs.find(i).is_some_and(|n| n.is_file())) {
                let ents = koei::linkdata_entries(&romfs.read_file(&idx).map_err(|e| e.to_string())?);
                let mut cur = vec![];
                let mut n = 0;
                for (i, e) in ents.into_iter().enumerate() {
                    n += e.2;
                    cur.push((i, e));
                    if n >= PACK_GROUP {
                        items.push(Item::Linkdata(ri, path.clone(), std::mem::take(&mut cur)));
                        n = 0;
                    }
                }
                if !cur.is_empty() {
                    items.push(Item::Linkdata(ri, path.clone(), cur));
                }
                return Ok(None);
            }
            if head.starts_with(capcom::KPKA) {
                let read = |o: u64, n: u64| romfs.read(&path, o, n).map_err(|e| e.to_string());
                let (ents, blocks) = capcom::kpka_entries(&read, &path)?;
                let blocks = Arc::new(blocks);
                let mut cur = vec![];
                let mut n = 0;
                for e in ents {
                    n += e.stored;
                    cur.push(e);
                    if n >= PACK_GROUP {
                        items.push(Item::Kpka(ri, path.clone(), std::mem::take(&mut cur), blocks.clone()));
                        n = 0;
                    }
                }
                if !cur.is_empty() {
                    items.push(Item::Kpka(ri, path.clone(), cur, blocks));
                }
                return Ok(None);
            }
            if head.starts_with(unreal::TOC_MAGIC) {
                tocs.push((ri, path.clone()));
                return Ok(None);
            }
            if size >= 0x2d {
                let tail = romfs.read(&path, size - size.min(0xe1), size.min(0xe1)).unwrap_or_default();
                if [0x2cusize, 0xc8, 0xcc].iter().any(|&b| tail.len() >= b && u32_at(&tail, tail.len() - b) == Some(unreal::PAK_MAGIC)) {
                    let read = |o: u64, n: u64| romfs.read(&path, o, n).map_err(|e| e.to_string());
                    let mut key = None;
                    if let Ok(f) = unreal::pak_footer(&read, size) {
                        if f.encrypted {
                            let first: [u8; 16] = read(f.index.0, 16)?.try_into().map_err(|_| "index cut short")?;
                            key = main().and_then(|(img, lo)| unreal::find_key(img, *lo, &first, &ue_keys));
                            if let Some(k) = key {
                                if !ue_keys.contains(&k) {
                                    ue_keys.push(k);
                                }
                                pak_keys.insert((ri, path.clone()), k);
                            }
                        }
                    }
                    let ents = match unreal::pak_files(&read, size, key.as_ref()) {
                        Ok(e) => {
                            ue_archives |= e.iter().any(|x| x.name.ends_with(".ushaderbytecode"));
                            unreal::shader_files(e)
                        }
                        Err(e) => {
                            // a pak is not read whole: its shaders are left out
                            pak_problems.push(format!("/{}{path}: Unreal pak: {e}; not scanned", roms[ri].0));
                            return Ok(None);
                        }
                    };
                    let mut cur = vec![];
                    let mut n = 0;
                    for e in ents {
                        n += e.size;
                        cur.push(e);
                        if n >= PACK_GROUP {
                            items.push(Item::Pak(ri, path.clone(), std::mem::take(&mut cur), key));
                            n = 0;
                        }
                    }
                    if !cur.is_empty() {
                        items.push(Item::Pak(ri, path.clone(), cur, key));
                    }
                    return Ok(None);
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
    // IoStore containers: shader libraries, and the cooked packages where no pak has shader code archives and the
    // container no library (shaders inline); their .ucas files are read through them
    let mut taken: std::collections::HashSet<(usize, String)> = Default::default();
    for (ri, path) in tocs {
        let romfs = roms[ri].1;
        let label = format!("/{}{path}", roms[ri].0);
        let toc = match romfs.read_file(&path).map_err(|e| e.to_string()).and_then(|b| unreal::Toc::parse(&b, &label)) {
            Ok(t) => Arc::new(t),
            Err(e) => {
                pak_problems.push(format!("{e}; not scanned"));
                continue;
            }
        };
        let base = path.strip_suffix(".utoc").unwrap_or(&path).to_string();
        let key = if toc.encrypted { pak_keys.get(&(ri, format!("{base}.pak"))).or(ue_keys.first()).copied() } else { None };
        if toc.encrypted && key.is_none() {
            pak_problems.push(format!("{label}: the container is encrypted and no key of the game opens a pak; not scanned"));
            continue;
        }
        let libs = toc.shader_libraries();
        let bundles = if libs.is_empty() && !ue_archives { toc.export_bundles() } else { vec![] };
        if libs.is_empty() && bundles.is_empty() {
            continue;
        }
        for k in 0..64u64 {
            taken.insert((ri, if k == 0 { format!("{base}.ucas") } else { format!("{base}_s{k}.ucas") }));
        }
        if !libs.is_empty() {
            items.push(Item::Io(ri, path.clone(), toc.clone(), libs, true, key));
        }
        let mut cur = vec![];
        let mut n = 0;
        for i in bundles {
            n += toc.ranges[i].1;
            cur.push(i);
            if n >= PACK_GROUP {
                items.push(Item::Io(ri, path.clone(), toc.clone(), std::mem::take(&mut cur), false, key));
                n = 0;
            }
        }
        if !cur.is_empty() {
            items.push(Item::Io(ri, path.clone(), toc.clone(), cur, false, key));
        }
    }
    items.retain(|it| !matches!(it, Item::File(ri, p, _) if taken.contains(&(*ri, p.clone()))));
    rep.problems.extend(pak_problems);
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
        Item::Bdt(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for &(h, off, n) in ents {
                let label = format!("/{pre}{path}#{h:08x}");
                match romfs.read(path, off, n).map_err(|e| e.to_string()).and_then(|d| {
                    bytes.fetch_add(n, Ordering::Relaxed);
                    fromsoft::bnd(&d, &label)
                }) {
                    Ok(f) => out.found.extend(f),
                    Err(e) => out.problems.push(e),
                }
            }
        }
        Item::Smash(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                if e.3 == 0 {
                    continue;
                }
                let at = format!("/{pre}{path}#{:010x}", e.0);
                match smash::read_file(&read, e, &at) {
                    Ok(d) => {
                        bytes.fetch_add(e.2, Ordering::Relaxed);
                        let label = smash::label_of(&format!("/{pre}{path}"), &d, e.0);
                        let n = out.found.len();
                        formats::scan_blob(ctx, &d, &label, out, 0);
                        if out.found.len() == n && memchr::memmem::find(&d, &formats::glslc::MAGIC).is_some() {
                            out.found.extend(formats::glslc::shaders(&d, &label));
                        }
                    }
                    Err(e) => out.problems.push(e),
                }
            }
        }
        Item::Rdb(ri, path, size) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            bytes.fetch_add(*size, Ordering::Relaxed);
            koei::rdb(&read, *size, &format!("/{pre}{path}"), out);
        }
        Item::Linkdata(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for &(i, (off, n, stored, comp)) in ents {
                if n == 0 {
                    continue;
                }
                let label = format!("/{pre}{path}#{i}");
                match romfs.read(path, off, if comp != 0 { stored } else { n }) {
                    Ok(d) => {
                        bytes.fetch_add(d.len() as u64, Ordering::Relaxed);
                        formats::scan_file(ctx, &d, &label, out);
                    }
                    Err(e) => out.problems.push(format!("{label}: {e}")),
                }
            }
        }
        Item::Kpka(ri, path, ents, blocks) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}#{:016x}", e.hash);
                bytes.fetch_add(e.stored, Ordering::Relaxed);
                match capcom::kpka_member(&read, e, blocks.as_ref().as_ref(), &label) {
                    Ok(f) => out.found.extend(f),
                    Err(e) => out.problems.push(e),
                }
            }
        }
        Item::Io(ri, path, toc, chunks, libs, key) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let base = path.strip_suffix(".utoc").unwrap_or(path);
            let read = |part: u64, o: u64, n: u64| {
                let f = if part == 0 { format!("{base}.ucas") } else { format!("{base}_s{part}.ucas") };
                romfs.read(&f, o, n).map_err(|e| e.to_string())
            };
            let index: HashMap<[u8; 12], usize> = if *libs { toc.ids.iter().enumerate().map(|(i, id)| (*id, i)).collect() } else { HashMap::new() };
            for &i in chunks {
                let label = format!("/{pre}{path}#{}", toc.ids[i].iter().map(|b| format!("{b:02x}")).collect::<String>());
                let d = match toc.read(i, &read, key.as_ref(), &label) {
                    Ok(d) => d,
                    Err(e) => {
                        out.problems.push(format!("{e}; not scanned"));
                        continue;
                    }
                };
                bytes.fetch_add(d.len() as u64, Ordering::Relaxed);
                if *libs {
                    let chunk = |id: &[u8; 12]| -> Result<Vec<u8>, String> {
                        let j = *index.get(id).ok_or("in no chunk of this container")?;
                        toc.read(j, &read, key.as_ref(), &label)
                    };
                    match unreal::io_library(&d, &chunk, &label) {
                        Ok(f) => out.found.extend(f),
                        Err(e) => out.problems.push(e),
                    }
                } else {
                    unreal::pak_member(&d, &label, &label, out);
                }
            }
        }
        Item::Pak(ri, path, ents, key) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}/{}", e.name);
                match unreal::pak_read(&read, e, &label, key.as_ref()) {
                    Ok(d) => {
                        bytes.fetch_add(e.size, Ordering::Relaxed);
                        unreal::pak_member(&d, &e.name, &label, out);
                    }
                    Err(e) => out.problems.push(format!("{e}; not scanned")),
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

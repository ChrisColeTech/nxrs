//! A whole game's shaders: every code module of its ExeFS and every file of its RomFS (the update applied), read in
//! place through the translator's package API, in parallel, one copy kept per key.

use crate::formats::{self, archive, capcom, engines, fromsoft, gamefreak, koei, monolith, nlg, packs, retro, smash, studios, unreal, Ctx, Scanned};
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
    /// A Next Level Games index (.dict), read with the files beside it.
    Nlg(usize, String),
    /// Entries of a Monolith .ard (path, offset, stored size), from its .arh.
    Arh(usize, String, Vec<(String, u64, u64)>),
    /// Members of a KIT archive (the index's path; data in `<path>.<part>`).
    Kit(usize, String, Vec<packs::KitEntry>),
    /// A Double Eleven resource pack (.pkdswitch, its index beside it).
    D11(usize, String, u64),
    /// A UbiArt IPK archive's .fxb effect files.
    Ipk(usize, String, u64),
    /// Entries of a PlatinumGames PKZ.
    Pkz(usize, String, Vec<packs::PkzEntry>),
    /// Files of a Rockstar RPF6 archive, with the key that opens it.
    Rpf(usize, String, Option<[u8; 32]>, Vec<packs::RpfFile>),
    /// Files of Camelot's CRA archive.
    Cra(usize, String, Vec<packs::CraEntry>),
    /// Files of one CASC archive (its path, its folder).
    Casc(usize, String, String, Vec<packs::CascEntry>),
    /// A byte range of a Diablo III package (false) or a PDAT archive (true).
    Range(usize, String, u64, u64, bool),
    /// Members of a zip archive.
    Zip(usize, String, Vec<packs::ZipEntry>),
    /// Members of a Larian LSPK package (its first part's path).
    Lspk(usize, String, Vec<packs::LspkEntry>),
    /// `.nnx_shaders` members of a TT Games CC4 DAT.
    TtDat(usize, String, Vec<formats::ttgames::TtEntry>),
    /// Members of a Virtuos SRYK (VBF) archive.
    Vbf(usize, String, Vec<packs::VbfEntry>),
    /// CompiledBytecode resources of a Frostbite cas file: (offset, stored, size, name).
    Fb(usize, String, Vec<(u64, u64, u64, String)>),
    /// A Retro RFRM pack read through its ADIR table.
    Rfrm(usize, String, u64),
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
            Item::Nlg(..) => 1 << 24,
            Item::Arh(_, _, e) => e.iter().map(|x| x.2).sum(),
            Item::Kit(_, _, e) => e.iter().map(|x| x.3).sum(),
            Item::D11(_, _, n) | Item::Ipk(_, _, n) | Item::Rfrm(_, _, n) => *n,
            Item::Pkz(_, _, e) => e.iter().map(|x| x.3).sum(),
            Item::Rpf(_, _, _, e) => e.iter().map(|x| x.stored).sum(),
            Item::Cra(_, _, e) => e.iter().map(|x| x.2).sum(),
            Item::Casc(_, _, _, e) => e.iter().map(|x| x.3).sum(),
            Item::Range(_, _, a, b, _) => b - a,
            Item::Zip(_, _, e) => e.iter().map(|x| x.stored).sum(),
            Item::Lspk(_, _, e) => e.iter().map(|x| x.3).sum(),
            Item::TtDat(_, _, e) => e.iter().map(|x| x.2).sum(),
            Item::Vbf(_, _, e) => e.iter().map(|x| x.1).sum(),
            Item::Fb(_, _, e) => e.iter().map(|x| x.1).sum(),
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
    let main_segments = || -> Option<(&[u8], Vec<(usize, usize)>)> {
        let (img, _) = main()?;
        let nso = exefs.get("main")?;
        let segs = (0..3).map(|i| Some((u32_at(nso, 0x14 + 0x10 * i)? as usize, u32_at(nso, 0x18 + 0x10 * i)? as usize))).collect::<Option<Vec<_>>>()?;
        Some((img.as_slice(), segs))
    };
    let rpf_key: std::cell::OnceCell<Option<[u8; 32]>> = std::cell::OnceCell::new();
    // NROs the game decodes at run time, captured from a boot (Camelot's: they hold the CRA index and shaders of their
    // own): NXSHADERS_NRO_DIR, scanned like main
    let nros: Vec<(String, Vec<u8>)> = std::env::var_os("NXSHADERS_NRO_DIR")
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|rd| {
            let mut v: Vec<(String, Vec<u8>)> = rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "nro")).filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).ok()?))).collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    for (n, b) in &nros {
        items.push(Item::Code(format!("main/{n}"), Arc::new(b.clone())));
    }
    let mut ue_keys: Vec<[u8; 32]> = vec![];
    let mut pak_keys: HashMap<(usize, String), [u8; 32]> = HashMap::new();
    let mut tocs: Vec<(usize, String)> = vec![];
    let mut ue_archives = false;
    for (ri, (_, romfs)) in roms.iter().enumerate() {
        let romfs = *romfs;
        let files = romfs.walk();
        rep.files += files.len();
        let pre = roms[ri].0.clone();
        let (taken_here, folder_items, folder_problems) = folder_formats(ri, &pre, romfs, &files);
        items.extend(folder_items);
        rep.problems.extend(folder_problems);
        let have: std::collections::HashSet<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        for (path, size) in files.iter().cloned() {
        if taken_here.contains(&path) {
            continue;
        }
        // read through its Next Level Games index
        if let Some((stem, _)) = nlg::SIBLINGS.iter().find_map(|e| path.strip_suffix(e).map(|s| (s, e))) {
            if have.contains(format!("{stem}.dict").as_str()) {
                continue;
            }
        }
        let head = romfs.read(&path, 0, 16).unwrap_or_default();
        let plan = (|| -> Result<Option<Item>, String> {
            let read = |o: u64, n: u64| romfs.read(&path, o, n).map_err(|e| e.to_string());
            let label = format!("/{pre}{path}");
            if head.starts_with(&nlg::DICT) {
                return Ok(Some(Item::Nlg(ri, path.clone())));
            }
            if path.ends_with(".arh") && monolith::is_arh(&head) {
                let ard = format!("{}.ard", &path[..path.len() - 4]);
                if !have.contains(ard.as_str()) {
                    return Err("ARH index has no ARD data file".into());
                }
                let ents = monolith::arh_entries(&romfs.read_file(&path).map_err(|e| e.to_string())?, &label)?;
                let ard_size = files.iter().find(|(p, _)| *p == ard).map(|x| x.1).unwrap_or(0);
                if ents.iter().any(|e| e.1 > ard_size || e.2 > ard_size - e.1) {
                    return Err("ARD entry out of bounds".into());
                }
                group(ents, |e| e.2, |c| Item::Arh(ri, ard.clone(), c), &mut items);
                return Ok(None);
            }
            if path.ends_with(".kit") && head.starts_with(packs::KIT) {
                let idx = romfs.read_file(&path).map_err(|e| e.to_string())?;
                let part = |n: u16| files.iter().find(|(p, _)| *p == format!("{path}.{n}")).map(|x| x.1);
                let ents = packs::kit_entries(&idx, &part, &label)?;
                group(ents, |e| e.3, |c| Item::Kit(ri, path.clone(), c), &mut items);
                return Ok(None);
            }
            if let Some(stem) = path.strip_suffix(".pkdswitch").filter(|s| have.contains(format!("{s}.pkiswitch").as_str())) {
                let _ = stem;
                return Ok(Some(Item::D11(ri, path.clone(), size)));
            }
            if head.starts_with(&packs::UBIART_IPK) {
                return Ok(Some(Item::Ipk(ri, path.clone(), size)));
            }
            if head.starts_with(packs::PKZL) && path.ends_with(".pkz") {
                let ents = packs::pkz_entries(&read, &label)?;
                group(ents, |e| e.3, |c| Item::Pkz(ri, path.clone(), c), &mut items);
                return Ok(None);
            }
            if head.starts_with(packs::RPF6) {
                let (_, _, enc) = packs::rpf_header(&head).ok_or("RPF6 header cut short")?;
                let key = if enc {
                    let first = read(16, 16)?;
                    let k = rpf_key.get_or_init(|| main_segments().and_then(|(img, segs)| packs::rpf_find_key(img, &segs, &first)));
                    Some((*k).ok_or("an encrypted RPF6 archive, and no 32-byte window of main decrypts its first entry to the root directory")?)
                } else {
                    None
                };
                let ents = packs::rpf_files(&read, size, key.as_ref(), &label)?;
                group(ents, |e| e.stored, |c| Item::Rpf(ri, path.clone(), key, c), &mut items);
                return Ok(None);
            }
            if head.starts_with(packs::CRA) {
                let idx = nros.iter().find_map(|(_, n)| packs::cra_index(n).ok()).ok_or_else(|| format!("a CRA archive, and no NRO in NXSHADERS_NRO_DIR ({}) holds its file table", std::env::var("NXSHADERS_NRO_DIR").unwrap_or_default()))?;
                group(idx, |e| e.2, |c| Item::Cra(ri, path.clone(), c), &mut items);
                return Ok(None);
            }
            if path.to_lowercase().ends_with(".cpk") && packs::is_d3cpk(&head) {
                let step = (size / 64).max(1 << 24);
                let mut a = 0;
                while a < size {
                    items.push(Item::Range(ri, path.clone(), a, (a + step).min(size), false));
                    a += step;
                }
                return Ok(None);
            }
            if head.starts_with(packs::PDAT) {
                let step = (size / 64).max(1 << 24);
                let mut a = 0;
                while a < size {
                    items.push(Item::Range(ri, path.clone(), a, (a + step).min(size), true));
                    a += step;
                }
                return Ok(None);
            }
            if head.starts_with(packs::ZIP) || head.starts_with(packs::VZIP) {
                match packs::zip_entries(&read, size, &label) {
                    Ok(ents) => {
                        group(ents, |e| e.size, |c| Item::Zip(ri, path.clone(), c), &mut items);
                        return Ok(None);
                    }
                    Err(e) => return Err(format!("the zip cannot be read ({e})")),
                }
            }
            if head.starts_with(b"SRYK") {
                let ents: Vec<packs::VbfEntry> = packs::vbf_entries(&read, &label)?.into_iter().filter(|e| packs::vbf_wanted(&e.0)).collect();
                group(ents, |e| e.1, |c| Item::Vbf(ri, path.clone(), c), &mut items);
                return Ok(None);
            }
            if head.starts_with(retro::RFRM) && size > 1 << 28 {
                return Ok(Some(Item::Rfrm(ri, path.clone(), size)));
            }
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

/// Entries in work items of about PACK_GROUP bytes.
fn group<T>(ents: Vec<T>, w: impl Fn(&T) -> u64, mk: impl Fn(Vec<T>) -> Item, items: &mut Vec<Item>) {
    let mut cur = vec![];
    let mut n = 0;
    for e in ents {
        n += w(&e);
        cur.push(e);
        if n >= PACK_GROUP {
            items.push(mk(std::mem::take(&mut cur)));
            n = 0;
        }
    }
    if !cur.is_empty() {
        items.push(mk(cur));
    }
}

/// Formats that span files or folders: Blizzard's CASC folders, Frostbite's tables, Larian's LSPK parts and the KIT
/// and ARH data files read through their indexes. → (paths they take, which a loose scan leaves alone; their work
/// items; problems)
fn folder_formats(ri: usize, pre: &str, romfs: &RomFs, files: &[(String, u64)]) -> (std::collections::HashSet<String>, Vec<Item>, Vec<String>) {
    let mut taken = std::collections::HashSet::new();
    let mut items = vec![];
    let mut problems = vec![];
    let dir_of = |p: &str| p.rfind('/').map_or(String::new(), |i| p[..i].to_string());
    let base_of = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    // CASC: a folder with build_key.txt, a manifest (.psv) and an archive (<chunk>-<archive>.data)
    let mut dirs: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    for (p, n) in files {
        dirs.entry(dir_of(p)).or_default().push((p.clone(), *n));
    }
    for (d, fs) in &dirs {
        let names: Vec<String> = fs.iter().map(|(p, _)| base_of(p)).collect();
        let casc = names.iter().any(|n| n == "build_key.txt")
            && names.iter().any(|n| n.ends_with(".psv"))
            && names.iter().any(|n| n.ends_with(".data") && n.len() == 16 && n.as_bytes()[2] == 0x2d);
        if !casc {
            continue;
        }
        let at = |n: &str| if d.is_empty() { n.to_string() } else { format!("{d}/{n}") };
        let r = (|| -> R<()> {
            let key = String::from_utf8_lossy(&romfs.read_file(&at("build_key.txt")).map_err(|e| e.to_string())?).into_owned();
            let mut psv = vec![];
            for n in names.iter().filter(|n| n.ends_with(".psv")) {
                psv.push((n.clone(), String::from_utf8_lossy(&romfs.read_file(&at(n)).map_err(|e| e.to_string())?).into_owned()));
            }
            psv.sort();
            let es = packs::casc_files(&key, &psv, &format!("/{pre}{d}"))?;
            let mut by: BTreeMap<String, Vec<packs::CascEntry>> = BTreeMap::new();
            for e in es {
                by.entry(e.1.clone()).or_default().push(e);
            }
            if let Some(lost) = by.keys().find(|a| !names.contains(a)) {
                return Err(format!("/{pre}{d}: the manifest names archive {lost}, which the RomFS lacks"));
            }
            for (a, mut es) in by {
                es.sort_by_key(|e| e.2);
                group(es, |e| e.3, |c| Item::Casc(ri, at(&a), d.clone(), c), &mut items);
            }
            for n in names.iter().filter(|n| n.ends_with(".data")) {
                taken.insert(at(n));
            }
            Ok(())
        })();
        if let Err(e) = r {
            problems.push(format!("{e}; its archives are scanned as loose files"));
        }
    }
    // Frostbite: <root>Data/layout.toc with the toc magic
    for (p, _) in files.iter().filter(|(p, _)| p == "Data/layout.toc" || p.ends_with("/Data/layout.toc")) {
        let root = &p[..p.len() - "Data/layout.toc".len()];
        let r = (|| -> R<()> {
            let layout = romfs.read_file(p).map_err(|e| e.to_string())?;
            if layout.get(..4) != Some(&formats::frostbite::TOC_MAGIC[..]) {
                return Ok(());
            }
            let chunks = formats::frostbite::install_chunks(&layout)?;
            let mut tocs = vec![];
            for (q, _) in files.iter().filter(|(q, _)| q.starts_with(&format!("{root}Data/NX/")) || q.starts_with(&format!("{root}Patch/NX/"))) {
                if q.ends_with(".toc") {
                    tocs.push((q.clone(), romfs.read_file(q).map_err(|e| e.to_string())?));
                }
            }
            let read = |path: &str, o: u64, n: u64| romfs.read(&format!("{root}{path}"), o, n).map_err(|e| e.to_string());
            let res = formats::frostbite::resources(&chunks, &tocs, &read, formats::frostbite::name_hash("CompiledBytecode"))?;
            let mut by: BTreeMap<String, Vec<(u64, u64, u64, String)>> = BTreeMap::new();
            for ((cas, off, stored), (size, name)) in res {
                by.entry(format!("{root}{cas}")).or_default().push((off, stored, size, name));
            }
            for (cas, es) in by {
                group(es, |e| e.1, |c| Item::Fb(ri, cas.clone(), c), &mut items);
            }
            for (q, _) in files.iter().filter(|(q, _)| q.starts_with(root) && (q.ends_with(".toc") || q.ends_with(".cas"))) {
                taken.insert(q.clone());
            }
            Ok(())
        })();
        if let Err(e) = r {
            problems.push(format!("/{pre}{p}: Frostbite: {e}; not scanned"));
        }
    }
    // TT Games' CC4 DATs (Skywalker Saga): only their indexed .nnx_shaders members are read
    for (p, size) in files.iter().filter(|(p, _)| p.to_lowercase().ends_with(".dat")) {
        if taken.contains(p) {
            continue;
        }
        let read = |o: u64, n: u64| romfs.read(p, o, n).map_err(|e| e.to_string());
        match formats::ttgames::entries(&read, *size, &format!("/{pre}{p}")) {
            Ok(None) => {}
            Ok(Some(ents)) => {
                taken.insert(p.clone());
                group(ents, |e| e.2, |c| Item::TtDat(ri, p.clone(), c), &mut items);
            }
            Err(e) => {
                taken.insert(p.clone());
                problems.push(format!("{e}; not scanned"));
            }
        }
    }
    // Larian LSPK packages (footer-indexed, in numbered parts): only their BSHD members are read
    for (p, size) in files.iter().filter(|(p, _)| p.to_lowercase().ends_with(".pak")) {
        if taken.contains(p) {
            continue;
        }
        let read = |o: u64, n: u64| romfs.read(p, o, n).map_err(|e| e.to_string());
        match packs::lspk_package(&read, *size, &format!("/{pre}{p}")) {
            Ok(None) => {}
            Ok(Some((parts, ents))) => {
                for i in 1..parts {
                    taken.insert(packs::lspk_part(p, i));
                }
                taken.insert(p.clone());
                let ents: Vec<packs::LspkEntry> = ents.into_iter().filter(|e| e.0.to_lowercase().ends_with(".bshd")).collect();
                group(ents, |e| e.3, |c| Item::Lspk(ri, p.clone(), c), &mut items);
            }
            Err(e) => {
                taken.insert(p.clone());
                problems.push(format!("{e}; not scanned"));
            }
        }
    }
    // the data files of KIT and ARH indexes
    for (p, _) in files {
        if let Some(stem) = p.strip_suffix(".arh") {
            if romfs.read(p, 0, 4).is_ok_and(|h| monolith::is_arh(&h)) {
                taken.insert(format!("{stem}.ard"));
            }
        }
        if p.ends_with(".kit") && romfs.read(p, 0, 4).is_ok_and(|h| h.starts_with(packs::KIT)) {
            for (q, _) in files.iter().filter(|(q, _)| q.strip_prefix(p.as_str()).is_some_and(|r| r.len() > 1 && r.starts_with('.') && r[1..].bytes().all(|b| b.is_ascii_digit()))) {
                taken.insert(q.clone());
            }
        }
    }
    (taken, items, problems)
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
        Item::Nlg(ri, path) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let label = format!("/{pre}{path}");
            let stem = path.strip_suffix(".dict").unwrap_or(path).to_string();
            let sib = |ext: &str| -> Option<Vec<u8>> {
                let p = format!("{stem}{ext}");
                romfs.find(&p).filter(|n| n.is_file())?;
                let b = romfs.read_file(&p).ok()?;
                bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
                Some(b)
            };
            match romfs.read_file(path).map_err(|e| e.to_string()).and_then(|idx| nlg::nlg(ctx, &idx, &sib, &label, out)) {
                Ok(()) => {}
                Err(e) => out.problems.push(format!("{e}; its shaders are left out")),
            }
        }
        Item::Arh(ri, ard, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let base = format!("/{pre}{ard}");
            for (name, off, n) in ents {
                let label = format!("{base}{name}");
                match romfs.read(ard, *off, *n) {
                    Ok(d) => {
                        bytes.fetch_add(d.len() as u64, Ordering::Relaxed);
                        if let Err(e) = monolith::shaders(&d, &label, &mut out.found, 0) {
                            out.problems.push(e);
                        }
                    }
                    Err(e) => out.problems.push(format!("{label}: {e}")),
                }
            }
        }
        Item::Kit(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for e in ents {
                let label = format!("/{pre}{path}#{}", e.0);
                let part = format!("{path}.{}", e.1);
                let read = |o: u64, n: u64| romfs.read(&part, o, n).map_err(|e| e.to_string());
                match packs::kit_read(&read, e, &label) {
                    Ok(d) => {
                        bytes.fetch_add(e.3, Ordering::Relaxed);
                        formats::scan_blob(ctx, &d, &label, out, 0);
                    }
                    Err(x) => out.problems.push(x),
                }
            }
        }
        Item::D11(ri, path, _) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let label = format!("/{pre}{path}");
            let idx = format!("{}.pkiswitch", path.strip_suffix(".pkdswitch").unwrap_or(path));
            let r = (|| -> R<()> {
                let (i, d) = (romfs.read_file(&idx).map_err(|e| e.to_string())?, romfs.read_file(path).map_err(|e| e.to_string())?);
                bytes.fetch_add((i.len() + d.len()) as u64, Ordering::Relaxed);
                for (h, m) in packs::d11_members(&i, &d, &label)? {
                    let lb = format!("{label}#{h:08x}");
                    let n = out.found.len();
                    formats::scan_blob(ctx, &m, &lb, out, 0);
                    if out.found.len() == n {
                        out.found.extend(formats::glslc::shaders(&m, &lb));
                    }
                }
                Ok(())
            })();
            if let Err(e) = r {
                out.problems.push(e);
            }
        }
        Item::Ipk(ri, path, _) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let label = format!("/{pre}{path}");
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            match packs::ipk_fxb(&read, &label) {
                Ok(ms) => {
                    for (name, b) in ms {
                        bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
                        let lb = format!("{label}/{name}");
                        let n = out.found.len();
                        formats::scan_blob(ctx, &b, &lb, out, 0);
                        if out.found.len() == n {
                            out.found.extend(formats::glslc::shaders(&b, &lb));
                        }
                    }
                }
                Err(e) => out.problems.push(e),
            }
        }
        Item::Pkz(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}/{}", e.0);
                match packs::pkz_read(&read, e, &label) {
                    Ok(d) => {
                        bytes.fetch_add(e.3, Ordering::Relaxed);
                        if e.1 == "OodleKraken" {
                            oodle.fetch_add(1, Ordering::Relaxed);
                        }
                        formats::scan_blob(ctx, &d, &label, out, 0);
                    }
                    Err(x) => out.problems.push(format!("{x}; not scanned")),
                }
            }
        }
        Item::Rpf(ri, path, key, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}/{}", e.name);
                match packs::rpf_read(&read, e, key.as_ref(), &label) {
                    Ok(d) => {
                        bytes.fetch_add(e.stored, Ordering::Relaxed);
                        formats::scan_blob(ctx, &d, &label, out, 0);
                    }
                    Err(x) => out.problems.push(format!("{x}; not scanned")),
                }
            }
        }
        Item::Cra(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}/{}", e.1);
                // sound and movies (FSTM, EBML) are left out by their first block
                if packs::cra_head(&read, e).is_ok_and(|h| h.starts_with(b"FSTM") || h.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])) {
                    continue;
                }
                match packs::cra_read(&read, e) {
                    Ok(d) => {
                        bytes.fetch_add(e.2, Ordering::Relaxed);
                        match packs::cra_undo(&d, 0) {
                            Ok(u) => formats::scan_blob(ctx, &u, &label, out, 0),
                            Err(x) => out.problems.push(format!("{label}: {x}")),
                        }
                    }
                    Err(x) => out.problems.push(format!("{label}: {x}; not scanned")),
                }
            }
        }
        Item::Casc(ri, archive, dir, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for e in ents {
                let label = format!("/{pre}{dir}/{}", e.0);
                match romfs.read(archive, e.2, e.3).map_err(|x| x.to_string()).and_then(|d| {
                    if d.len() as u64 != e.3 {
                        return Err(format!("{} bytes at {:#x} run past the archive's end", e.3, e.2));
                    }
                    packs::casc_decode(&d, &e.5, e.4, &label)
                }) {
                    Ok(d) => {
                        bytes.fetch_add(e.3, Ordering::Relaxed);
                        formats::scan_blob(ctx, &d, &label, out, 0);
                    }
                    Err(x) => out.problems.push(format!("{label}: {x}; not scanned")),
                }
            }
        }
        Item::Range(ri, path, a, b, pdat) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let label = format!("/{pre}{path}");
            let span = if *pdat { packs::PDAT_SPAN } else { packs::D3_SPAN };
            match romfs.read(path, *a, b - a + span) {
                Ok(d) => {
                    bytes.fetch_add(b - a, Ordering::Relaxed);
                    if *pdat {
                        match packs::pdat_shaders(&d, *a, *b, &label) {
                            Ok(f) => out.found.extend(f),
                            Err(e) => out.problems.push(e),
                        }
                    } else {
                        let f = packs::d3_shaders(&d, *a, *b, &label, &mut out.problems);
                        out.found.extend(f);
                    }
                }
                Err(e) => out.problems.push(format!("{label}: {e}")),
            }
        }
        Item::Zip(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}/{}", e.name);
                match packs::zip_read(&read, e, &label) {
                    Ok(d) => {
                        bytes.fetch_add(e.stored, Ordering::Relaxed);
                        let n = out.found.len();
                        formats::scan_blob(ctx, &d, &label, out, 0);
                        if out.found.len() == n && d.get(..4) != Some(b"MSHD") && memchr::memmem::find(&d, &formats::glslc::MAGIC).is_some() {
                            out.found.extend(formats::glslc::shaders(&d, &label));
                        }
                    }
                    Err(x) => out.problems.push(format!("{x}; not scanned")),
                }
            }
        }
        Item::Lspk(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for e in ents {
                let part = packs::lspk_part(path, e.1);
                let label = format!("/{pre}{path}/{}", e.0);
                let read = |o: u64, n: u64| romfs.read(&part, o, n).map_err(|e| e.to_string());
                match packs::lspk_read(&read, e, &label).and_then(|d| studios::bshd(&d, &label)) {
                    Ok(f) => {
                        bytes.fetch_add(e.3, Ordering::Relaxed);
                        out.found.extend(f);
                    }
                    Err(x) => out.problems.push(x),
                }
            }
        }
        Item::TtDat(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}/{}", e.0);
                match formats::ttgames::read_member(&read, e, &label) {
                    Ok(d) => {
                        bytes.fetch_add(e.2, Ordering::Relaxed);
                        let f = formats::ttgames::programs(&d, &label, &mut out.problems);
                        out.found.extend(f);
                    }
                    Err(x) => out.problems.push(x),
                }
            }
        }
        Item::Vbf(ri, path, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            for e in ents {
                let label = format!("/{pre}{path}/{}", e.0);
                match packs::vbf_read(&read, e, &label) {
                    Ok(d) => {
                        bytes.fetch_add(e.1, Ordering::Relaxed);
                        out.found.extend(studios::phyre(&d, &label));
                    }
                    Err(x) => out.problems.push(x),
                }
            }
        }
        Item::Fb(ri, cas, ents) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            for (off, stored, size, name) in ents {
                let label = format!("/{pre}{cas}#{name}");
                let r = romfs.read(cas, *off, *stored).map_err(|e| e.to_string()).and_then(|d| {
                    bytes.fetch_add(*stored, Ordering::Relaxed);
                    let item = formats::frostbite::read_item(&d, *size, &label)?;
                    oodle.fetch_add(1, Ordering::Relaxed);
                    formats::frostbite::nvn_shader(&item).map_err(|e| format!("{label}: {e}"))
                });
                match r {
                    Ok(Some((code, ctl))) => {
                        let st = formats::glslc::stage(&ctl, &code);
                        match st.map(|st| (st, check(st, &ctl, &code))) {
                            Some((st, None)) => out.found.push(Found::new(label, st, &ctl, &code)),
                            Some((_, Some(w))) => out.problems.push(format!("{label}: {w}; left out")),
                            None => out.problems.push(format!("{label}: no stage in the code area header; left out")),
                        }
                    }
                    Ok(None) => out.problems.push(format!("{label}: a CompiledBytecode resource without a VNVN record; left out")),
                    Err(e) => out.problems.push(e),
                }
            }
        }
        Item::Rfrm(ri, path, size) => {
            let (pre, romfs) = (&roms[*ri].0, roms[*ri].1);
            let label = format!("/{pre}{path}");
            let read = |o: u64, n: u64| romfs.read(path, o, n).map_err(|e| e.to_string());
            bytes.fetch_add(*size, Ordering::Relaxed);
            match retro::rfrm_read(&read, *size, &label) {
                Ok(f) => out.found.extend(f),
                Err(e) => out.problems.push(e),
            }
        }
    }
}

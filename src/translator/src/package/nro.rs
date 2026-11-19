//! NRO modules a game loads itself at run time (`nn::ro::LoadModule`): found at build time so they are translated
//! with the ExeFS (nxnative's `nro_collect.py`). Every SHA-256 the RomFS's NRR files (`.nrr/*.nrr`) register is
//! looked for among the RomFS's loose files that are NROs (`NRO0` at +0x10). The game hands LoadModule the bytes it
//! read; the runtime finds the translated copy by that SHA-256, so the names are labels only: `nro_<index>` in hash
//! order. An online SDK (Epic Online Services, known by its export `EOS_Platform_Create`) is left out and listed as
//! refused: the runtime refuses to load it and the game plays offline.
use crate::package::romfs::RomFs;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The largest NRO collected (an image this big is not a code module).
pub const MAX_NRO: u64 = 256 << 20;

pub struct Nro {
    /// `nro_<index>`
    pub name: String,
    /// The RomFS path it was read from.
    pub path: String,
    pub sha256: [u8; 32],
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Collected {
    pub nros: Vec<Nro>,
    /// Registered hashes no RomFS file holds (a game may decode its NRO at run time).
    pub missing: Vec<[u8; 32]>,
    /// Online SDKs left out: (RomFS path, SHA-256, build id).
    pub refused: Vec<(String, [u8; 32], String)>,
    /// NRR files read and the hashes they register.
    pub nrrs: usize,
    pub registered: usize,
}

/// The hashes an NRR registers (version 1, the Switch 2's, has a header 0x180 bytes longer).
pub fn nrr_hashes(d: &[u8]) -> Vec<[u8; 32]> {
    if d.len() < 0x350 || &d[..4] != b"NRR0" {
        return vec![];
    }
    let hdr = if d[5] == 1 { 0x4c0 } else { 0x340 };
    let Some(f) = d.get(hdr..hdr + 8) else { return vec![] };
    let (off, cnt) = (u32::from_le_bytes(f[..4].try_into().unwrap()) as usize, u32::from_le_bytes(f[4..].try_into().unwrap()) as usize);
    (0..cnt).filter_map(|i| d.get(off + 32 * i..off + 32 * i + 32)).map(|h| h.try_into().unwrap()).collect()
}

/// Does the NRO define `name` in its dynamic symbol table? (a plain search of its .dynstr; enough to recognise an
/// online SDK by an export)
fn exports(nro: &[u8], name: &str) -> bool {
    let needle = format!("\0{name}\0");
    nro.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// The first `n` bytes of a zstd frame (an archive member's header without inflating it all).
pub fn zstd_head(d: &[u8], n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut dec = zstd::stream::read::Decoder::with_buffer(d).ok()?;
    dec.window_log_max(31).ok()?;
    let mut out = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match dec.read(&mut out[got..]) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(_) => return None,
        }
    }
    out.truncate(got);
    Some(out)
}

/// A RomFS file read from an offset on (for a zstd decoder).
struct FileReader<'a> {
    romfs: &'a RomFs,
    path: &'a str,
    at: u64,
}

impl std::io::Read for FileReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let d = self.romfs.read(self.path, self.at, buf.len().min(1 << 20) as u64).map_err(std::io::Error::other)?;
        buf[..d.len()].copy_from_slice(&d);
        self.at += d.len() as u64;
        Ok(d.len())
    }
}

/// NROs stored inside the RomFS's larger files, found by a scan (Super Smash Bros. Ultimate's data.arc, whose
/// index this reader does not parse in every version): every NRO header stored as is (`NRO0` at +0x10) and every
/// zstd frame whose first bytes inflate to one; the ones whose SHA-256 is in `want` are returned as (label, bytes).
/// `say` reports progress. nxnative's nro_collect.py scans the same way when an archive has no index it reads.
pub fn scan(romfs: &RomFs, want: &[[u8; 32]], say: &dyn Fn(&str)) -> crate::package::error::Result<Vec<(String, Vec<u8>)>> {
    use std::io::Read;
    const WIN: u64 = 64 << 20;
    const ZMAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
    let mut out: Vec<(String, Vec<u8>)> = vec![];
    let mut seen: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
    let files: Vec<(String, u64)> = romfs.walk().into_iter().filter(|(_, n)| *n >= (1 << 20)).collect();
    let total: u64 = files.iter().map(|f| f.1).sum();
    let mut done = 0u64;
    let t0 = std::time::Instant::now();
    for (p, size) in &files {
        let mut at = 0u64;
        while at < *size {
            let n = WIN.min(size - at);
            let w = romfs.read(p, at, n + 0x40000)?;
            let mut i = 0usize;
            while i + 4 <= n as usize {
                let z = w[i..i + 4] == ZMAGIC;
                let stored = i >= 0x10 && &w[i..i + 4] == b"NRO0";
                if !(z || stored) {
                    i += 1;
                    continue;
                }
                let start = if z { i } else { i - 0x10 };
                let head = if z { zstd_head(&w[start..(start + 0x40000).min(w.len())], 0x40) } else { w.get(start..start + 0x40).map(|h| h.to_vec()) };
                if let Some(h) = head.filter(|h| h.len() >= 0x40 && &h[0x10..0x14] == b"NRO0") {
                    let len = u32::from_le_bytes(h[0x18..0x1c].try_into().unwrap()) as u64;
                    if (0x100..=MAX_NRO).contains(&len) {
                        let abs = at + start as u64;
                        let mut d = vec![];
                        let ok = if z {
                            zstd::stream::read::Decoder::new(FileReader { romfs, path: p, at: abs })
                                .and_then(|mut dec| {
                                    dec.window_log_max(31)?;
                                    (&mut dec).take(len).read_to_end(&mut d)
                                })
                                .is_ok()
                        } else {
                            d = romfs.read(p, abs, len)?;
                            true
                        };
                        if ok && d.len() as u64 == len {
                            let hash: [u8; 32] = Sha256::digest(&d).into();
                            if want.contains(&hash) && seen.insert(hash) {
                                out.push((format!("{p}@{abs:#x}"), d));
                                if seen.len() == want.len() {
                                    say(&format!("NRO scan: all {} found in {:.0} s", want.len(), t0.elapsed().as_secs_f64()));
                                    return Ok(out);
                                }
                            }
                        }
                    }
                }
                i += 4;
            }
            at += n;
            done += n;
            if (done - n) >> 30 == done >> 30 {
                continue;
            }
            say(&format!("NRO scan: {:.1} of {:.1} GB, {} of {} found ({:.0} s)", done as f64 / 1e9, total as f64 / 1e9, seen.len(), want.len(), t0.elapsed().as_secs_f64()));
        }
    }
    Ok(out)
}

/// Every registered NRO of the RomFS.
pub fn collect(romfs: &RomFs) -> crate::package::error::Result<Collected> {
    collect_with(romfs, &|_| Ok(vec![]))
}

/// The same, with `extra` asked for the registered hashes no loose file holds: NROs inside the game's archives
/// (Super Smash Bros. Ultimate's data.arc), as (label, bytes).
pub fn collect_with(romfs: &RomFs, extra: &dyn Fn(&[[u8; 32]]) -> crate::package::error::Result<Vec<(String, Vec<u8>)>>) -> crate::package::error::Result<Collected> {
    let mut out = Collected::default();
    let mut wanted: BTreeMap<[u8; 32], ()> = BTreeMap::new();
    let files = romfs.walk();
    for (p, size) in &files {
        if p.to_ascii_lowercase().ends_with(".nrr") && *size < (64 << 20) {
            let hs = nrr_hashes(&romfs.read_file(p)?);
            out.nrrs += 1;
            for h in hs {
                wanted.insert(h, ());
            }
        }
    }
    out.registered = wanted.len();
    if wanted.is_empty() {
        return Ok(out);
    }
    let mut found: BTreeMap<[u8; 32], (String, Vec<u8>)> = BTreeMap::new();
    for (p, size) in &files {
        if *size < 0x80 || *size > MAX_NRO {
            continue;
        }
        if romfs.read(p, 0x10, 4)? != b"NRO0" {
            continue;
        }
        let d = romfs.read_file(p)?;
        let n = u32::from_le_bytes(d[0x18..0x1c].try_into().unwrap()) as usize;
        let d = if n <= d.len() { d[..n].to_vec() } else { continue };
        let h: [u8; 32] = Sha256::digest(&d).into();
        if wanted.contains_key(&h) {
            found.entry(h).or_insert((p.clone(), d));
        }
    }
    let missing: Vec<[u8; 32]> = wanted.keys().filter(|h| !found.contains_key(*h)).copied().collect();
    if !missing.is_empty() {
        for (p, d) in extra(&missing)? {
            let h: [u8; 32] = Sha256::digest(&d).into();
            if wanted.contains_key(&h) {
                found.entry(h).or_insert((p, d));
            }
        }
    }
    for h in wanted.keys() {
        match found.remove(h) {
            Some((path, bytes)) if exports(&bytes, "EOS_Platform_Create") => {
                let b = bytes[0x40..0x60].iter().map(|b| format!("{b:02x}")).collect();
                out.refused.push((path, *h, b))
            }
            Some((path, bytes)) => {
                let name = format!("nro_{:03}", out.nros.len());
                out.nros.push(Nro { name, path, sha256: *h, bytes });
            }
            None => out.missing.push(*h),
        }
    }
    Ok(out)
}

/// `nro.tsv` for the runtime: `name<TAB>base<TAB>size<TAB>nro bytes<TAB>sha256<TAB>build id<TAB>RomFS path` per module
/// (base and size from the translator's layout), `refused<TAB>-<TAB>-<TAB>-<TAB>sha256<TAB>build id<TAB>path` per
/// online SDK. The runtime finds a module by its build id (the NRO header's +0x40) and checks its size.
pub fn tsv(c: &Collected, layout: &[(String, u64, u64)]) -> String {
    let hex = |h: &[u8; 32]| h.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let bid = |d: &[u8]| d.get(0x40..0x60).map_or(String::new(), |b| b.iter().map(|b| format!("{b:02x}")).collect());
    let mut s = String::from("# NRO modules the game loads at run time (nxrs-build): name, base, size, NRO bytes, SHA-256, build id, RomFS path\n");
    for n in &c.nros {
        if let Some((_, base, size)) = layout.iter().find(|m| m.0 == n.name) {
            s.push_str(&format!("{}\t{base:#x}\t{size:#x}\t{:#x}\t{}\t{}\t{}\n", n.name, n.bytes.len(), hex(&n.sha256), bid(&n.bytes), n.path));
        }
    }
    for (p, h, b) in &c.refused {
        s.push_str(&format!("refused\t-\t-\t-\t{}\t{b}\t{p}\n", hex(h)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nro(tag: u8, extra: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x100];
        d[0x10..0x14].copy_from_slice(b"NRO0");
        d[0x80] = tag;
        d.extend_from_slice(extra);
        let n = d.len() as u32;
        d[0x18..0x1c].copy_from_slice(&n.to_le_bytes());
        d
    }

    fn nrr(hashes: &[[u8; 32]]) -> Vec<u8> {
        let mut d = vec![0u8; 0x350];
        d[..4].copy_from_slice(b"NRR0");
        d[0x340..0x344].copy_from_slice(&0x350u32.to_le_bytes());
        d[0x344..0x348].copy_from_slice(&(hashes.len() as u32).to_le_bytes());
        for h in hashes {
            d.extend_from_slice(h);
        }
        d
    }

    #[test]
    fn registered_nros_are_collected_and_online_sdks_refused() {
        let (a, b, eos, stray) = (nro(1, b""), nro(2, b""), nro(3, b"\0EOS_Platform_Create\0"), nro(4, b""));
        let h = |d: &[u8]| -> [u8; 32] { Sha256::digest(d).into() };
        let gone = [9u8; 32];
        let r = nrr(&[h(&a), h(&b), h(&eos), gone]);
        let img = crate::package::romfs::build_image(&[
            (".nrr/game.nrr", &r),
            ("nro/a.nro", &a),
            ("nro/b.nro", &b),
            ("nro/eos.nro", &eos),
            ("nro/stray.nro", &stray),
        ]);
        let romfs = RomFs::from_image(std::sync::Arc::new(img), "test".into()).unwrap();
        let c = collect(&romfs).unwrap();
        assert_eq!((c.nrrs, c.registered), (1, 4));
        let mut names: Vec<&str> = c.nros.iter().map(|n| n.path.as_str()).collect();
        names.sort();
        assert_eq!(names, ["nro/a.nro", "nro/b.nro"]);
        assert_eq!(c.refused.len(), 1);
        assert_eq!(c.missing, vec![gone]);
        assert!(c.nros.iter().all(|n| n.name.starts_with("nro_")));
        let t = tsv(&c, &[("nro_000".into(), 0x7100000000, 0x2000)]);
        assert!(t.contains("nro_000\t0x7100000000\t0x2000\t0x100\t"));
        assert!(t.contains("refused\t-"));
    }
}

//! Frostbite (EA SPORTS FC 27): the CompiledBytecode resources of the bundles its superbundle tables (`.toc`) list,
//! stored in its cas files as blocks (stored, or Oodle Leviathan), each a VNVN record with one code area and control
//! section. `Data/layout.toc`'s installManifest names the install chunks the cas ids refer to.

use crate::formats::archive;
use crate::nvn::*;
use std::collections::{BTreeMap, HashMap};

pub const TOC_MAGIC: [u8; 4] = [0x00, 0xd1, 0xce, 0x01];
const TOC_BODY: usize = 0x22c;
const BUNDLE_MAGIC: u32 = 0x9d79_8ed6;

/// Frostbite's type name hash: djb2 with xor over the lower-case name.
pub fn name_hash(name: &str) -> u32 {
    name.to_lowercase()
        .bytes()
        .fold(5381u32, |h, c| h.wrapping_mul(33) ^ c as u32)
}

/// A DbObject value.
#[derive(Debug, Clone)]
pub enum Db {
    List(Vec<Db>),
    Obj(Vec<(String, Db)>),
    Bool(bool),
    Str(String),
    Int(i64),
    Float(f64),
    Bytes(Vec<u8>),
}

impl Db {
    fn get(&self, k: &str) -> Option<&Db> {
        match self {
            Db::Obj(v) => v.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
}

fn leb(d: &[u8], mut p: usize) -> R<(usize, usize)> {
    let (mut v, mut s) = (0usize, 0);
    loop {
        let b = *d.get(p).ok_or("DbObject: a length runs past the end")?;
        p += 1;
        v |= ((b & 0x7f) as usize) << s;
        s += 7;
        if b & 0x80 == 0 || s > 56 {
            return Ok((v, p));
        }
    }
}

/// One DbObject value at `p` → ((name, value), next offset).
pub fn dbobject(d: &[u8], mut p: usize, depth: usize) -> R<((String, Db), usize)> {
    if depth > 64 {
        return Err("DbObject: nested too deep".into());
    }
    let t = *d.get(p).ok_or("DbObject: past the end")?;
    p += 1;
    let mut name = String::new();
    if t & 0x80 == 0 {
        let e = d
            .get(p..)
            .and_then(|s| memchr::memchr(0, s))
            .ok_or("DbObject: a name runs past the end")?
            + p;
        name = String::from_utf8_lossy(&d[p..e]).into_owned();
        p = e + 1;
    }
    let k = t & 0x1f;
    let cut = || "DbObject: a value runs past the end".to_string();
    Ok(match k {
        1 | 2 => {
            let (n, q) = leb(d, p)?;
            let end = q + n;
            let mut q = q;
            let mut items = vec![];
            while q < end && *d.get(q).ok_or_else(cut)? != 0 {
                let (it, nq) = dbobject(d, q, depth + 1)?;
                items.push(it);
                q = nq;
            }
            (
                (
                    name,
                    if k == 2 {
                        Db::Obj(items)
                    } else {
                        Db::List(items.into_iter().map(|x| x.1).collect())
                    },
                ),
                end,
            )
        }
        7 => {
            let (n, q) = leb(d, p)?;
            let s = d.get(q..q + n.saturating_sub(1)).ok_or_else(cut)?;
            (
                (name, Db::Str(String::from_utf8_lossy(s).into_owned())),
                q + n,
            )
        }
        19 => {
            let (n, q) = leb(d, p)?;
            (
                (name, Db::Bytes(d.get(q..q + n).ok_or_else(cut)?.to_vec())),
                q + n,
            )
        }
        6 => ((name, Db::Bool(*d.get(p).ok_or_else(cut)? != 0)), p + 1),
        8 => (
            (
                name,
                Db::Int(
                    i32::from_le_bytes(d.get(p..p + 4).ok_or_else(cut)?.try_into().unwrap()) as i64,
                ),
            ),
            p + 4,
        ),
        9 => (
            (
                name,
                Db::Int(i64::from_le_bytes(
                    d.get(p..p + 8).ok_or_else(cut)?.try_into().unwrap(),
                )),
            ),
            p + 8,
        ),
        0xb => (
            (
                name,
                Db::Float(
                    f32::from_le_bytes(d.get(p..p + 4).ok_or_else(cut)?.try_into().unwrap()) as f64,
                ),
            ),
            p + 4,
        ),
        0xc => (
            (
                name,
                Db::Float(f64::from_le_bytes(
                    d.get(p..p + 8).ok_or_else(cut)?.try_into().unwrap(),
                )),
            ),
            p + 8,
        ),
        0xf | 0x10 => {
            let n = if k == 0xf { 16 } else { 20 };
            (
                (name, Db::Bytes(d.get(p..p + n).ok_or_else(cut)?.to_vec())),
                p + n,
            )
        }
        _ => {
            return Err(format!(
                "DbObject: type {t:#x} at {:#x} is not known",
                p - 1
            ))
        }
    })
}

/// {persistentIndex: the install chunk's folder under NX/} from Data/layout.toc.
pub fn install_chunks(layout: &[u8]) -> R<HashMap<u32, String>> {
    let ((_, lay), _) = dbobject(layout, TOC_BODY, 0)?;
    let chunks = lay
        .get("installManifest")
        .and_then(|m| m.get("installChunks"))
        .ok_or("Data/layout.toc: no installManifest")?;
    let Db::List(cs) = chunks else {
        return Err("Data/layout.toc: installChunks is not a list".into());
    };
    let mut out = HashMap::new();
    for c in cs {
        if let (Some(Db::Int(pi)), Some(Db::Str(n))) = (c.get("persistentIndex"), c.get("name")) {
            out.insert(*pi as u32, n.clone());
        }
    }
    Ok(out)
}

/// A superbundle table's bundles: per bundle its entries (cas id, offset, stored size), entry 0 the manifest.
pub fn toc_bundles(d: &[u8], what: &str) -> R<Vec<Vec<([u8; 8], u32, u32)>>> {
    if d.get(..4) != Some(&TOC_MAGIC[..]) {
        return Err(format!("{what}: not a Frostbite toc"));
    }
    if d.len() < TOC_BODY + 0x20 {
        return Ok(vec![]);
    }
    let be = |o: usize| -> R<u32> {
        d.get(o..o + 4)
            .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
            .ok_or_else(|| format!("{what}: cut short"))
    };
    let b = TOC_BODY;
    let (bo, nb) = (be(b + 4)? as usize, be(b + 8)? as usize);
    let mut out = vec![];
    for i in 0..nb {
        let o = b + bo + 16 * i;
        let sz = be(o + 4)? as usize;
        let off = ((be(o + 8)? as u64) << 32 | be(o + 12)? as u64) as usize;
        let s = b + off;
        let end = s + (sz & 0xff_ffff);
        if end > d.len() {
            return Err(format!("{what}: bundle {i}'s data runs past the end"));
        }
        let (fo, n, p) = (
            be(s + 8)? as usize + s,
            be(s + 12)? as usize,
            be(s + 16)? as usize + s,
        );
        if fo + n > end {
            return Err(format!("{what}: bundle {i}'s flags run past its data"));
        }
        let mut cas: Option<[u8; 8]> = None;
        let mut ents = vec![];
        let mut p = p;
        for k in 0..n {
            if d[fo + k] & 0x80 != 0 || cas.is_none() {
                cas = Some(
                    d.get(p..p + 8)
                        .ok_or_else(|| format!("{what}: cut short"))?
                        .try_into()
                        .unwrap(),
                );
                p += 8;
            }
            ents.push((cas.unwrap(), be(p)?, be(p + 4)?));
            p += 8;
        }
        if p != fo {
            return Err(format!(
                "{what}: bundle {i}'s entries end at {:#x}, its flags start at {:#x}",
                p - s,
                fo - s
            ));
        }
        out.push(ents);
    }
    Ok(out)
}

/// The cas file a cas id names (RomFS path).
pub fn cas_path(cid: &[u8; 8], chunks: &HashMap<u32, String>) -> R<String> {
    let pi = u32::from_be_bytes(cid[2..6].try_into().unwrap());
    let n = u16::from_be_bytes(cid[6..8].try_into().unwrap());
    let c = chunks.get(&pi).ok_or_else(|| {
        format!("cas id {cid:02x?}: no install chunk has persistentIndex {pi:#x}")
    })?;
    Ok(format!(
        "{}/NX/{c}/cas_{n:02}.cas",
        if cid[1] != 0 { "Patch" } else { "Data" }
    ))
}

/// A bundle manifest at the start of `d` → (its length, ebx count, res (name, size, type), chunk count).
fn manifest(d: &[u8]) -> R<(usize, usize, Vec<(String, u32, u32)>, usize)> {
    let hl = d
        .get(..4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or("bundle manifest cut short")? as usize;
    let w = |o: usize| u32_at(d, o).ok_or_else(|| "bundle manifest cut short".to_string());
    let (magic, tot, ne, nr, nc, so) = (
        w(4)?,
        w(8)? as usize,
        w(12)? as usize,
        w(16)? as usize,
        w(20)? as usize,
        w(24)? as usize,
    );
    if magic != BUNDLE_MAGIC {
        return Err(format!("no bundle manifest (magic {magic:#x})"));
    }
    if ne + nr + nc != tot {
        return Err(format!(
            "bundle manifest: {ne} + {nr} + {nc} items, not the {tot} it counts"
        ));
    }
    let h = 4;
    let mut p = h + 0x20 + 20 * tot;
    p += 8 * ne;
    let res: Vec<(u32, u32)> = (0..nr)
        .map(|i| Ok((w(p + 8 * i)?, w(p + 8 * i + 4)?)))
        .collect::<R<_>>()?;
    p += 8 * nr;
    let types: Vec<u32> = (0..nr).map(|i| w(p + 4 * i)).collect::<R<_>>()?;
    let name = |o: u32| -> R<String> {
        let a = h + so + o as usize;
        let e = d
            .get(a..(h + hl).min(d.len()))
            .and_then(|s| memchr::memchr(0, s))
            .ok_or("bundle manifest: a name runs past the manifest")?
            + a;
        Ok(String::from_utf8_lossy(&d[a..e]).into_owned())
    };
    let res = res
        .into_iter()
        .zip(types)
        .map(|((a, s), t)| Ok((name(a)?, s, t)))
        .collect::<R<Vec<_>>>()?;
    Ok((hl + 4, ne, res, nc))
}

/// Every res of type `rtype` the bundles of every superbundle hold → {(cas path, offset, stored size): (size, name)}.
/// `tocs`: every `.toc` under Data/NX and Patch/NX; `read(path, offset, n)` reads RomFS bytes.
pub fn resources(
    chunks: &HashMap<u32, String>,
    tocs: &[(String, Vec<u8>)],
    read: &dyn Fn(&str, u64, u64) -> R<Vec<u8>>,
    rtype: u32,
) -> R<BTreeMap<(String, u64, u64), (u64, String)>> {
    let mut out = BTreeMap::new();
    for (t, d) in tocs {
        for ents in toc_bundles(d, t)? {
            let (cid, off, size) = ents[0];
            let m = read(&cas_path(&cid, chunks)?, off as u64, size as u64)?;
            let (mlen, ne, res, nc) = manifest(&m).map_err(|e| format!("{t}: {e}"))?;
            if mlen != size as usize {
                return Err(format!(
                    "{t}: a bundle manifest of {mlen:#x} bytes, the toc says {size:#x}"
                ));
            }
            if ne + res.len() + nc != ents.len() - 1 {
                return Err(format!(
                    "{t}: a bundle of {} items with {} entries",
                    ne + res.len() + nc,
                    ents.len() - 1
                ));
            }
            for ((nm, sz, ty), (c, o, s)) in res.iter().zip(&ents[1 + ne..]) {
                if *ty == rtype {
                    out.insert(
                        (cas_path(c, chunks)?, *o as u64, *s as u64),
                        (*sz as u64, nm.clone()),
                    );
                }
            }
        }
    }
    Ok(out)
}

/// An item's data: its blocks {u32 BE size (low 24 bits), u8 codec, u8 0x70 | stored bits 16..19, u16 BE stored} decoded
/// (codec 0 stored, 0x19 Oodle).
pub fn read_item(d: &[u8], size: u64, label: &str) -> R<Vec<u8>> {
    let mut o = 0usize;
    let mut out = Vec::with_capacity(size as usize);
    while o < d.len() {
        if o + 8 > d.len() {
            return Err(format!(
                "{label}: a block header at {o:#x} runs past the item"
            ));
        }
        let n = (u32::from_be_bytes(d[o..o + 4].try_into().unwrap()) & 0xff_ffff) as usize;
        let (codec, cs) = (d[o + 4], d[o + 5]);
        let c = ((cs as usize & 0xf) << 16)
            | u16::from_be_bytes(d[o + 6..o + 8].try_into().unwrap()) as usize;
        if cs & 0xf0 != 0x70 || o + 8 + c > d.len() {
            return Err(format!("{label}: a damaged block at {o:#x}"));
        }
        let blk = &d[o + 8..o + 8 + c];
        match codec {
            0 => out.extend_from_slice(blk),
            0x19 => out.extend(archive::oodle(blk, n, label)?),
            x => {
                return Err(format!(
                    "{label}: block at {o:#x}: codec {x:#x} is not known"
                ))
            }
        }
        o += 8 + c;
    }
    if out.len() as u64 != size {
        return Err(format!(
            "{label}: {:#x} bytes decoded, not {size:#x}",
            out.len()
        ));
    }
    Ok(out)
}

/// A CompiledBytecode resource → (code area, control section), None without a VNVN record.
pub fn nvn_shader(b: &[u8]) -> R<Option<(Vec<u8>, Vec<u8>)>> {
    if b.len() < 0x24 || &b[0x20..0x24] != b"VNVN" {
        return Ok(None);
    }
    let r = 0x20;
    let v = u16_at(b, r + 8).ok_or("VNVN record cut short")?;
    let mut p = r + if v >= 3 { 0x11 } else { 0x10 };
    for _ in 0..4 {
        p += 2 + 4 * u16_at(b, p).ok_or("VNVN record cut short")? as usize;
    }
    p += 2 + 8 * u16_at(b, p).ok_or("VNVN record cut short")? as usize;
    let w = |o: usize| {
        u32_at(b, o)
            .map(|x| x as usize)
            .ok_or("VNVN record cut short")
    };
    let (co, cl, ko, kl) = (w(p)?, w(p + 4)?, w(p + 8)?, w(p + 12)?);
    if r + co + cl > b.len() || r + ko + kl > b.len() {
        return Err(format!(
            "VNVN record: code {co:#x}+{cl:#x} or control {ko:#x}+{kl:#x} outside its {:#x} bytes",
            b.len() - r
        ));
    }
    Ok(Some((
        b[r + co..r + co + cl].to_vec(),
        b[r + ko..r + ko + kl].to_vec(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_dbobject() {
        assert_eq!(name_hash("CompiledBytecode"), 0x0e2b_02f7);
        assert_eq!(name_hash("Texture"), 0x6bde_20ba);
        // {installManifest: {installChunks: [{name: "superbundlelayout/a", persistentIndex: 5}]}}
        let mut chunk = vec![];
        chunk.push(0x07);
        chunk.extend(b"name\0");
        let s = b"superbundlelayout/a\0";
        chunk.push(s.len() as u8);
        chunk.extend(s);
        chunk.push(0x08);
        chunk.extend(b"persistentIndex\0");
        chunk.extend(5i32.to_le_bytes());
        chunk.push(0);
        let mut obj = vec![0x82, chunk.len() as u8];
        obj.extend(&chunk);
        let mut list = vec![0x01];
        list.extend(b"installChunks\0");
        list.push(obj.len() as u8 + 1);
        list.extend(&obj);
        list.push(0);
        let mut im = vec![0x02];
        im.extend(b"installManifest\0");
        im.push(list.len() as u8 + 1);
        im.extend(&list);
        im.push(0);
        let mut root = vec![0x82, im.len() as u8 + 1];
        root.extend(&im);
        root.push(0);
        let mut layout = TOC_MAGIC.to_vec();
        layout.resize(TOC_BODY, 0);
        layout.extend(&root);
        let c = install_chunks(&layout).unwrap();
        assert_eq!(c[&5], "superbundlelayout/a");
        let cid = [0u8, 1, 0, 0, 0, 5, 0, 3];
        assert_eq!(
            cas_path(&cid, &c).unwrap(),
            "Patch/NX/superbundlelayout/a/cas_03.cas"
        );
        // a stored block
        let mut item = vec![0, 0, 0, 3, 0, 0x70, 0, 3];
        item.extend(b"abc");
        assert_eq!(read_item(&item, 3, "t").unwrap(), b"abc");
    }
}

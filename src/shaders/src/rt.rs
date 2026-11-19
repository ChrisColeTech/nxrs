//! NVN2 ray tracing: every stage-8 module of a game (control +0x56 = 8; its RTCore metadata at control +0x60,
//! +0x5c bytes) linked into one library: the modules' code concatenated at 256-byte boundaries, every entry region
//! given a logical ID (positive 16-bit, contiguous within an entry's continuations), the logical-ID MOV relocations
//! planned, and the named resources (`coreParams`, `descriptorBaseArray`) checked equal across modules. The translator
//! (job `rt`) turns the library into one compute kernel that runs the ray-generation entries and their continuations
//! and traces rays with ray queries (`SPV_KHR_ray_query` against the scene's acceleration structure).
//!
//! The metadata reader validates the whole serialized record (outer header, region tables, the inner records,
//! relocations); unknown numeric fields are kept, not interpreted.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The library's shader key (one per game).
pub const KEY: &str = "rt_library";
/// Part of the library's identity: a change here builds it again.
pub const VERSION: &str = "rt 1";
pub const LOCAL: [u32; 3] = [8, 4, 1];
const CODE_ALIGNMENT: usize = 256;
const LOGICAL_ID_MOV: i32 = -56;
const RESOURCES: [(&str, u64); 2] = [("coreParams", 0x140), ("descriptorBaseArray", 0x1430)];

type R<T> = Result<T, String>;

fn need(c: bool, m: &str) -> R<()> {
    if c {
        Ok(())
    } else {
        Err(m.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct Region {
    pub offset: usize,
    pub size: usize,
    pub name: String,
    pub selector: i64,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub fields: [u32; 9],
    pub regions: Vec<String>,
}

impl Entry {
    pub fn kind(&self) -> u32 {
        self.fields[8]
    }
    pub fn native_stack_bytes(&self) -> u32 {
        self.fields[1]
    }
    pub fn continuation_frame_bytes(&self) -> u32 {
        self.fields[2]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    pub name: String,
    pub kind: u32,
    pub size: u64,
    pub alignment: u64,
    pub offset: u64,
    pub data: Vec<u8>,
    pub fields: (u32, u64, u32),
}

#[derive(Debug, Clone)]
pub struct Relocation {
    pub patch_offset: u64,
    pub addend: u64,
    pub template: u64,
    pub kind: i32,
    pub region_base: u32,
    pub target: u32,
    pub tail: u32,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub header: [u32; 14],
    pub fingerprint: [u8; 16],
    pub regions: Vec<Region>,
    pub entry_region_counts: Vec<u32>,
    pub entries: Vec<Entry>,
    pub resources: Vec<Resource>,
    pub relocations: Vec<Relocation>,
    pub module160: u32,
    pub module_fields: (u32, u32, u32),
    pub metadata: Vec<u8>,
    pub code: Vec<u8>,
}

struct Rd<'a> {
    d: &'a [u8],
    pos: usize,
    end: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> R<&[u8]> {
        need(n <= self.end - self.pos, "truncated inner record")?;
        let v = &self.d[self.pos..self.pos + n];
        self.pos += n;
        Ok(v)
    }
    fn align(&mut self, a: usize) -> R<()> {
        let n = self.pos.wrapping_neg() & (a - 1);
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> R<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn count(&mut self, min: usize) -> R<usize> {
        self.align(8)?;
        let c = self.u64()? as usize;
        need(c <= (self.end - self.pos) / min.max(1), "inner count exceeds remaining input")?;
        Ok(c)
    }
    fn vector(&mut self, stride: usize) -> R<Vec<u8>> {
        let n = self.count(stride)?;
        Ok(self.take(n * stride)?.to_vec())
    }
    fn name(&mut self) -> R<String> {
        let raw = self.vector(1)?;
        need(!raw.is_empty() && !raw.contains(&0), "empty or embedded-NUL symbol name")?;
        String::from_utf8(raw).map_err(|_| "invalid UTF-8 symbol name".to_string())
    }
}

fn serialized_region(r: &mut Rd) -> R<String> {
    let name = r.name()?;
    r.align(4)?;
    r.take(17 * 4)?;
    r.align(8)?;
    r.take(8 * 8)?;
    for _ in 0..r.count(72)? {
        r.name()?;
        r.align(4)?;
        r.take(64)?;
    }
    for _ in 0..r.count(16)? {
        r.align(4)?;
        r.u32()?;
        r.align(8)?;
        r.u64()?;
    }
    r.u8()?;
    r.align(4)?;
    r.u32()?;
    r.vector(8)?;
    r.take(2)?;
    Ok(name)
}

fn entry(r: &mut Rd) -> R<Entry> {
    let name = r.name()?;
    r.align(4)?;
    let mut fields = [0u32; 9];
    for f in &mut fields {
        *f = r.u32()?;
    }
    r.take(9)?;
    r.align(4)?;
    r.u32()?;
    let n = r.count(80)?;
    let regions = (0..n).map(|_| serialized_region(r)).collect::<R<Vec<_>>>()?;
    need(fields[0] as usize == regions.len(), "entry region count disagrees with inner records")?;
    r.align(4)?;
    r.u32()?;
    r.vector(12)?;
    r.align(4)?;
    r.take(6)?;
    r.align(4)?;
    r.u32()?;
    Ok(Entry { name, fields, regions })
}

fn resource(r: &mut Rd) -> R<Resource> {
    let name = r.name()?;
    r.align(4)?;
    let kind = r.u32()?;
    r.align(8)?;
    let (size, alignment, offset) = (r.u64()?, r.u64()?, r.u64()?);
    need(alignment > 0 && alignment & (alignment - 1) == 0, "resource alignment is not a power of two")?;
    need(offset % alignment == 0, "misaligned resource offset")?;
    need(offset.checked_add(size).is_some(), "resource extent overflows u64")?;
    let data = r.vector(1)?;
    r.align(4)?;
    let f0 = r.u32()?;
    r.align(8)?;
    let (f1, f2) = (r.u64()?, r.u32()?);
    Ok(Resource { name, kind, size, alignment, offset, data, fields: (f0, f1, f2) })
}

#[allow(clippy::type_complexity)]
fn inner(m: &[u8], start: usize, size: usize) -> R<([u8; 16], Vec<Entry>, Vec<Resource>, u32, (u32, u32, u32))> {
    need(start <= m.len() && size <= m.len() - start, "inner module outside input")?;
    let mut r = Rd { d: m, pos: start, end: start + size };
    let fingerprint: [u8; 16] = r.take(16)?.try_into().unwrap();
    r.vector(1)?;
    r.align(8)?;
    r.take(0xc5)?;
    r.vector(1)?;
    r.take(3)?;
    r.align(4)?;
    r.u32()?;
    need(r.count(1)? == 0, "unsupported nonempty module parameter list")?;
    r.align(4)?;
    r.u32()?;
    r.name()?;
    r.align(8)?;
    let fixed = r.take(0x24)?.to_vec();
    let module160 = u32::from_le_bytes(fixed[8..12].try_into().unwrap());
    r.align(4)?;
    r.u32()?;
    for _ in 0..r.count(48)? {
        r.align(4)?;
        r.take(8)?;
        r.vector(8)?;
        r.vector(4)?;
        r.align(4)?;
        r.take(9)?;
        r.vector(24)?;
        r.vector(4)?;
    }
    let n = r.count(80)?;
    let entries = (0..n).map(|_| entry(&mut r)).collect::<R<Vec<_>>>()?;
    for _ in 0..r.count(20)? {
        r.name()?;
        r.align(4)?;
        r.take(12)?;
    }
    r.u8()?;
    for _ in 0..2 {
        for _ in 0..r.count(16)? {
            r.name()?;
            r.align(8)?;
            r.u64()?;
        }
    }
    for _ in 0..r.count(16)? {
        r.name()?;
        for _ in 0..r.count(8)? {
            r.name()?;
        }
    }
    r.align(4)?;
    let (m250, m254) = (r.u32()?, r.u32()?);
    let n = r.count(56)?;
    let resources = (0..n).map(|_| resource(&mut r)).collect::<R<Vec<_>>>()?;
    r.u8()?;
    r.align(4)?;
    let m278 = r.u32()?;
    r.vector(1)?;
    need(r.pos == r.end, "unconsumed inner metadata variant")?;
    Ok((fingerprint, entries, resources, module160, (m250, m254, m278)))
}

/// Validate and read one module's metadata (control +0x60, +0x5c bytes) with its code area.
pub fn parse_module(metadata: &[u8], code: &[u8]) -> R<Module> {
    need(!code.is_empty() && code.len() % 16 == 0, "code is empty or not 16-byte aligned")?;
    need(metadata.len() >= 56, "outer header outside input")?;
    let mut h = [0u32; 14];
    for (i, x) in h.iter_mut().enumerate() {
        *x = u32::from_le_bytes(metadata[4 * i..4 * i + 4].try_into().unwrap());
    }
    need(h[3] >= 56 && h[3] % 8 == 0, "misaligned inner metadata")?;
    need(h[4] > 0, "invalid outer region count")?;
    need(0 < h[11] && h[11] <= h[4], "invalid outer entry count")?;
    let words = |off: u32, count: u32, label: &str| -> R<Vec<u32>> {
        let (off, count) = (off as usize, count as usize);
        need(off % 4 == 0, &format!("misaligned {label}"))?;
        need(off <= metadata.len() && count <= (metadata.len() - off) / 4, &format!("{label} count exceeds input"))?;
        Ok((0..count).map(|i| u32::from_le_bytes(metadata[off + 4 * i..off + 4 * i + 4].try_into().unwrap())).collect())
    };
    let (starts, sizes, names, selectors) = (words(h[5], h[4], "region starts")?, words(h[6], h[4], "region sizes")?, words(h[7], h[4], "region names")?, words(h[8], h[4], "region selectors")?);
    let entry_counts = words(h[12], h[11], "entry region counts")?;
    need(entry_counts.iter().all(|&c| c != 0) && entry_counts.iter().sum::<u32>() == h[4], "entry regions do not cover region table")?;
    let mut regions = vec![];
    for i in 0..h[4] as usize {
        let (offset, size) = (starts[i] as usize, sizes[i] as usize);
        need(offset % 16 == 0 && size % 16 == 0 && size > 0, "unaligned or empty region")?;
        need(offset <= code.len() && size <= code.len() - offset, "region code outside input")?;
        let no = names[i] as usize;
        need(no < metadata.len(), "region name outside metadata")?;
        let end = memchr::memchr(0, &metadata[no..]).map(|e| e + no);
        let end = end.filter(|&e| e > no).ok_or("empty or unterminated region name")?;
        let name = String::from_utf8(metadata[no..end].to_vec()).map_err(|_| "invalid UTF-8 region name")?;
        regions.push(Region { offset, size, name, selector: selectors[i] as i32 as i64 });
    }
    let (fingerprint, entries, resources, module160, module_fields) = inner(metadata, h[3] as usize, h[2] as usize)?;
    need(entries.len() == h[11] as usize, "inner/outer entry count mismatch")?;
    need(entries.iter().map(|e| e.regions.len() as u32).eq(entry_counts.iter().copied()), "inner/outer region count mismatch")?;
    need(entries.iter().flat_map(|e| e.regions.iter()).eq(regions.iter().map(|r| &r.name)), "inner/outer region names disagree")?;
    let (ro, rn) = (h[10] as usize, h[9] as usize);
    need(ro <= metadata.len() && rn * 40 <= metadata.len() - ro, "relocation table outside input")?;
    need(ro % 8 == 0, "misaligned relocation table")?;
    let mut relocations = vec![];
    for i in 0..rn {
        let o = ro + 40 * i;
        let q = |k: usize| u64::from_le_bytes(metadata[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
        let w = |k: usize| u32::from_le_bytes(metadata[o + 24 + 4 * k..o + 28 + 4 * k].try_into().unwrap());
        let r = Relocation { patch_offset: q(0), addend: q(1), template: q(2), kind: w(0) as i32, region_base: w(1), target: w(2), tail: w(3) };
        let sources: Vec<&Region> = regions.iter().filter(|g| g.offset as u64 == r.region_base as u64).collect();
        need(!sources.is_empty(), "relocation base has no region")?;
        need(r.patch_offset % 16 == 0 && sources.iter().any(|g| r.patch_offset + 16 <= g.size as u64), "relocation instruction outside region")?;
        let at = r.region_base as u64 + r.patch_offset;
        need(at + 16 <= code.len() as u64, "relocation instruction outside input")?;
        if r.kind < 0 {
            need((r.target as usize) < regions.len(), "logical relocation target outside module")?;
        } else {
            need((r.target as usize) < code.len(), "code relocation target outside module")?;
        }
        relocations.push(r);
    }
    Ok(Module { header: h, fingerprint, regions, entry_region_counts: entry_counts, entries, resources, relocations, module160, module_fields, metadata: metadata.to_vec(), code: code.to_vec() })
}

/// One region given its logical ID: (ID, entry index, region index).
#[derive(Debug, Clone)]
pub struct Linked {
    pub id: u32,
    pub entry: usize,
    pub region: usize,
}

/// The module's regions given `ids` (contiguous within an entry), its relocations checked: → (regions, moves
/// (instruction offset, template, target ID)).
pub fn link_plan(m: &Module, ids: &[u32], fingerprint: &[u8; 16]) -> R<(Vec<Linked>, Vec<(u64, u64, u32)>)> {
    need(ids.len() == m.regions.len(), "logical ID count differs from region count")?;
    need(ids.iter().all(|&i| 0 < i && i < 0x8000), "logical ID outside positive signed16 domain")?;
    need(&m.fingerprint == fingerprint, "link fingerprint mismatch")?;
    need(m.header[13] <= 1, "unsupported outer metadata flag")?;
    need(m.module_fields == (0, 3, 7), "unsupported executable module fields")?;
    need(m.regions.iter().all(|r| r.selector == -1), "unsupported internal region selector")?;
    need(m.entries.iter().all(|e| matches!(e.kind(), 0 | 7 | 8)), "unsupported executable entry kind")?;
    need(m.resources.iter().all(|r| r.kind == 3), "unsupported executable resource kind")?;
    need(m.resources.iter().all(|r| r.data.len() as u64 <= r.size), "resource initializer exceeds extent")?;
    let mut sorted: Vec<&Region> = m.regions.iter().collect();
    sorted.sort_by_key(|r| r.offset);
    need(sorted.windows(2).all(|w| w[0].offset + w[0].size <= w[1].offset), "unsupported overlapping executable regions")?;
    let mut linked = vec![];
    let mut cur = 0usize;
    for (ei, &c) in m.entry_region_counts.iter().enumerate() {
        let part = &ids[cur..cur + c as usize];
        need(part.iter().enumerate().all(|(k, &x)| x == part[0] + k as u32), "continuation IDs are not contiguous")?;
        for ri in cur..cur + c as usize {
            linked.push(Linked { id: ids[ri], entry: ei, region: ri });
        }
        cur += c as usize;
    }
    let mut moves = vec![];
    let mut sites = std::collections::HashSet::new();
    for r in &m.relocations {
        need(r.kind == LOGICAL_ID_MOV, &format!("unsupported executable relocation kind {}", r.kind))?;
        need(r.addend == 0 && r.tail == 0, "unsupported logical MOV addend or trailing field")?;
        need(r.template < 1 << 32 && matches!(r.template & 0xfff, 0x802 | 0x882), "unsupported logical MOV template")?;
        let at = r.region_base as u64 + r.patch_offset;
        let src = u64::from_le_bytes(m.code[at as usize..at as usize + 8].try_into().unwrap());
        need(src == r.template, "logical MOV template disagrees with original instruction")?;
        need(sites.insert(at), "duplicate relocation instruction")?;
        moves.push((at, r.template, ids[r.target as usize]));
    }
    Ok((linked, moves))
}

/// One stage-8 module's metadata from its control section, checked against its code.
pub fn module_of(control: &[u8], code: &[u8]) -> R<Module> {
    let u = |o: usize| control.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
    need(control.len() >= 0x68, "truncated RT control header")?;
    need(u(0) == 0x9876_1234 && u(4) == 0xc9, "unsupported RT control schema")?;
    need(control[0x56] == 8, "RT control is not stage 8")?;
    let (size, offset, code_size) = (u(0x5c) as usize, u(0x60) as usize, u(0x64) as usize);
    need(size > 0 && offset >= 0x68 && offset % 8 == 0 && offset <= control.len() && size <= control.len() - offset, "RT metadata extent outside control")?;
    need(code_size == code.len() && u(0x44) as usize == code.len(), "RT control code size disagrees with code bytes")?;
    let m = parse_module(&control[offset..offset + size], code)?;
    need(m.module160 == 0 && m.entries.iter().all(|e| e.native_stack_bytes() == 0), "unsupported native RT stack requirement")?;
    need(m.entries.iter().all(|e| e.kind() == 0 || e.regions.len() == 1), "unsupported non-raygen continuation regions")?;
    need(m.entries.iter().all(|e| e.continuation_frame_bytes() <= 0x10000), "RT continuation frame exceeds 64 KiB")?;
    Ok(m)
}

/// The linked library: its code, the translator's `rt` job record and the catalog the reflection carries.
pub struct Library {
    pub code: Vec<u8>,
    pub rt: Value,
    pub manifest: Value,
    pub local_memory_bytes: u32,
}

fn sha(d: &[u8]) -> String {
    Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect()
}

/// Link every RT module (key, control, code) into one library; identical repeated keys collapse.
pub fn make_library(records: &[(String, Vec<u8>, Vec<u8>)]) -> R<Library> {
    let mut uniq: std::collections::BTreeMap<u64, (&[u8], &[u8])> = Default::default();
    for (k, c, d) in records {
        let key = u64::from_str_radix(k, 16).map_err(|_| "RT key must be sixteen hex digits".to_string())?;
        if let Some(prev) = uniq.get(&key) {
            need(prev.0 == &c[..] && prev.1 == &d[..], &format!("conflicting RT records for key {key:016x}"))?;
        }
        uniq.insert(key, (c, d));
    }
    need(!uniq.is_empty(), "RT library has no modules")?;
    let mut image: Vec<u8> = vec![];
    let (mut modules, mut moves, mut entries_json) = (vec![], vec![], vec![]);
    let mut fingerprint: Option<[u8; 16]> = None;
    let mut layout: Option<Vec<Resource>> = None;
    let mut next_id = 1u32;
    let (mut raygen_frame, mut callback_frame) = (-1i64, 0i64);
    for (key, (control, code)) in &uniq {
        let m = module_of(control, code)?;
        need(next_id as usize + m.regions.len() <= 0x8000, "RT library exceeds positive signed16 logical IDs")?;
        let fp = *fingerprint.get_or_insert(m.fingerprint);
        let ids: Vec<u32> = (next_id..next_id + m.regions.len() as u32).collect();
        let (linked, mv) = link_plan(&m, &ids, &fp)?;
        let mut cur: Vec<Resource> = m.resources.clone();
        cur.sort_by(|a, b| (a.offset, &a.name).cmp(&(b.offset, &b.name)));
        let names: std::collections::BTreeSet<&str> = cur.iter().map(|r| r.name.as_str()).collect();
        need(names == RESOURCES.iter().map(|x| x.0).collect(), "unsupported or missing RT resource symbols")?;
        need(cur.iter().all(|r| r.kind == 3 && RESOURCES.iter().any(|(n, s)| *n == r.name && *s == r.size)), "unsupported named RT resource extent or kind")?;
        need(cur.windows(2).all(|w| w[0].offset + w[0].size <= w[1].offset), "RT resource extents overlap")?;
        let l = layout.get_or_insert_with(|| cur.clone());
        need(*l == cur, "RT modules disagree on named resource layout")?;
        let address = image.len().div_ceil(CODE_ALIGNMENT) * CODE_ALIGNMENT;
        image.resize(address, 0);
        image.extend_from_slice(code);
        let mut owner: std::collections::HashMap<usize, u32> = Default::default();
        let mut ents = vec![];
        for lk in &linked {
            let e = &m.entries[lk.entry];
            let o = *owner.entry(lk.entry).or_insert(lk.id);
            let rec = json!({"address": address + m.regions[lk.region].offset, "id": lk.id, "owner": o, "kind": e.kind(), "frame": e.continuation_frame_bytes()});
            entries_json.push(rec.clone());
            let mut full = rec;
            full["entry_index"] = json!(lk.entry);
            full["region_index"] = json!(lk.region);
            ents.push(full);
        }
        for e in &m.entries {
            if e.kind() == 0 {
                raygen_frame = raygen_frame.max(e.continuation_frame_bytes() as i64);
            } else {
                callback_frame = callback_frame.max(e.continuation_frame_bytes() as i64);
            }
        }
        modules.push(json!({"key": format!("{key:016x}"), "address": address, "length": code.len(), "control_sha256": sha(control), "metadata_sha256": sha(&m.metadata), "code_sha256": sha(code), "entries": ents}));
        moves.extend(mv.into_iter().map(|(at, t, id)| (address as u64 + at, t, id)));
        next_id += m.regions.len() as u32;
    }
    need(raygen_frame >= 0, "RT library has no ray-generation entry")?;
    let arena = ((32i64.max(raygen_frame + callback_frame) + 31) & !31) as u32;
    need(arena <= 0x10000, "RT continuation arena exceeds 64 KiB")?;
    moves.sort();
    let layout = layout.unwrap();
    let off = |n: &str| layout.iter().find(|r| r.name == n).unwrap().offset;
    let rt = json!({
        "core_offset": off("coreParams"),
        "descriptor_offset": off("descriptorBaseArray"),
        "modules": modules.iter().map(|m| json!({"address": m["address"], "length": m["length"]})).collect::<Vec<_>>(),
        "entries": entries_json,
        "moves": moves.iter().map(|(a, t, id)| json!({"address": a, "template": t, "id": id})).collect::<Vec<_>>(),
    });
    let manifest = json!({
        "version": 1,
        "fingerprint": fingerprint.unwrap().iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "code_alignment": CODE_ALIGNMENT,
        "code_bytes": image.len(),
        "code_sha256": sha(&image),
        "local_memory_bytes": arena,
        "bank_bytes": layout.iter().map(|r| r.offset + r.size).max().unwrap_or(0),
        "resources": layout.iter().map(|r| json!({"name": r.name, "kind": r.kind, "size": r.size, "alignment": r.alignment, "offset": r.offset, "data": r.data.iter().map(|b| format!("{b:02x}")).collect::<String>(), "fields": [r.fields.0, r.fields.1, r.fields.2]})).collect::<Vec<_>>(),
        "modules": modules,
        "rt": rt.clone(),
    });
    Ok(Library { code: image, rt, manifest, local_memory_bytes: arena })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A writer of the serialized record, as RTCore lays it out.
    struct W {
        d: Vec<u8>,
    }
    impl W {
        fn align(&mut self, n: usize) {
            let k = self.d.len().wrapping_neg() & (n - 1);
            self.d.extend(vec![0u8; k]);
        }
        fn u8(&mut self, v: u8) {
            self.d.push(v);
        }
        fn u32(&mut self, v: u32) {
            self.d.extend(v.to_le_bytes());
        }
        fn u64(&mut self, v: u64) {
            self.d.extend(v.to_le_bytes());
        }
        fn count(&mut self, n: u64) {
            self.align(8);
            self.u64(n);
        }
        fn blob(&mut self, b: &[u8]) {
            self.count(b.len() as u64);
            self.d.extend(b);
        }
        fn region(&mut self, name: &str) {
            self.blob(name.as_bytes());
            self.align(4);
            for i in 0..17 {
                self.u32(i);
            }
            self.align(8);
            for i in 0..8 {
                self.u64(i);
            }
            self.count(1);
            self.blob(b"read");
            self.align(4);
            for i in 0..16 {
                self.u32(i);
            }
            self.count(1);
            self.u32(17);
            self.align(8);
            self.u64(0x1_0000_0003);
            self.u8(0);
            self.align(4);
            self.u32(0);
            self.count(1);
            self.u32(19);
            self.u32(23);
            self.u8(0);
            self.u8(0);
        }
        fn resource(&mut self, name: &str, size: u64, offset: u64) {
            self.blob(name.as_bytes());
            self.align(4);
            self.u32(3);
            self.align(8);
            self.u64(size);
            self.u64(16);
            self.u64(offset);
            self.u64(0);
            self.align(4);
            self.u32(11);
            self.align(8);
            self.u64(22);
            self.u32(33);
        }
    }

    /// A stage-8 module: one ray-generation entry with two regions (start, resume) and one logical-ID MOV.
    pub fn fixture() -> (Vec<u8>, Vec<u8>) {
        let mut w = W { d: vec![] };
        w.d.extend(0..16u8);
        w.blob(b"");
        w.align(8);
        w.d.extend(vec![0u8; 0xc5]);
        w.blob(b"");
        w.d.extend([0, 0, 0]);
        w.align(4);
        w.u32(0);
        w.count(0);
        w.align(4);
        w.u32(0);
        w.blob(b"synthetic_module");
        w.align(8);
        w.d.extend(vec![0u8; 0x24]);
        w.align(4);
        w.u32(0);
        w.count(0);
        w.count(1);
        // entry
        w.blob(b"entry0");
        w.align(4);
        for v in [2u32, 0, 96, 0, 0, 0, 0, 0, 0] {
            w.u32(v);
        }
        w.d.extend([0u8; 9]);
        w.align(4);
        w.u32(0);
        w.count(2);
        w.region("start");
        w.region("resume");
        w.align(4);
        w.u32(0);
        w.count(0);
        w.align(4);
        w.u32(0);
        w.u8(0);
        w.u8(0);
        w.align(4);
        w.u32(0);
        w.count(0);
        w.u8(0);
        w.count(0);
        w.count(0);
        w.count(0);
        w.align(4);
        w.u32(0);
        w.u32(3);
        w.count(2);
        w.resource("coreParams", 0x140, 0);
        w.resource("descriptorBaseArray", 0x1430, 0x140);
        w.u8(0);
        w.align(4);
        w.u32(7);
        w.blob(b"");
        let inner = w.d;
        let mut o = W { d: vec![0u8; 56] };
        o.d.extend(&inner);
        let names = ["start", "resume"];
        let mut h = [0u32, 0, inner.len() as u32, 56, 2, 0, 0, 0, 0, 1, 0, 1, 0, 0];
        for (idx, vals) in [(5, vec![0u32, 32]), (6, vec![32, 32]), (7, vec![0, 0]), (8, vec![u32::MAX, u32::MAX])] {
            o.align(4);
            h[idx] = o.d.len() as u32;
            for v in vals {
                o.u32(v);
            }
        }
        let mut pos = vec![];
        for n in names {
            pos.push(o.d.len() as u32);
            o.d.extend(n.as_bytes());
            o.d.push(0);
        }
        for (i, p) in pos.iter().enumerate() {
            let at = h[7] as usize + 4 * i;
            o.d[at..at + 4].copy_from_slice(&p.to_le_bytes());
        }
        o.align(8);
        h[10] = o.d.len() as u32;
        o.u64(16);
        o.u64(0);
        o.u64(0x27802);
        o.u32(LOGICAL_ID_MOV as u32);
        o.u32(0);
        o.u32(1);
        o.u32(0);
        o.align(4);
        h[12] = o.d.len() as u32;
        o.u32(2);
        for (i, v) in h.iter().enumerate() {
            o.d[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut code = vec![0u8; 64];
        code[16..24].copy_from_slice(&0x27802u64.to_le_bytes());
        code[24..32].copy_from_slice(&0x000f_e400_0000_0f00u64.to_le_bytes());
        // the control section: header, the metadata at +0xe0
        let mut ctl = vec![0u8; 0xe0];
        ctl[..4].copy_from_slice(&0x9876_1234u32.to_le_bytes());
        ctl[4..8].copy_from_slice(&0xc9u32.to_le_bytes());
        ctl[0x30..0x34].copy_from_slice(&(0xe0u32 + o.d.len() as u32).to_le_bytes());
        ctl[0x44..0x48].copy_from_slice(&(code.len() as u32).to_le_bytes());
        ctl[0x56] = 8;
        ctl[0x5c..0x60].copy_from_slice(&(o.d.len() as u32).to_le_bytes());
        ctl[0x60..0x64].copy_from_slice(&0xe0u32.to_le_bytes());
        ctl[0x64..0x68].copy_from_slice(&(code.len() as u32).to_le_bytes());
        ctl.extend(&o.d);
        (ctl, code)
    }

    #[test]
    fn parse_and_link() {
        let (ctl, code) = fixture();
        let m = module_of(&ctl, &code).unwrap();
        assert_eq!(m.regions.len(), 2);
        assert_eq!(m.entries[0].continuation_frame_bytes(), 96);
        assert_eq!(m.resources[1].name, "descriptorBaseArray");
        assert_eq!(crate::nvn::check(crate::nvn::Stage::Rt, &ctl, &code), None);
        let lib = make_library(&[("00000000000000aa".into(), ctl.clone(), code.clone()), ("00000000000000bb".into(), ctl, code)]).unwrap();
        // two modules at 256-byte boundaries, IDs 1..4, the MOVs naming region "resume" of each
        assert_eq!(lib.code.len(), 256 + 64);
        assert_eq!(lib.rt["entries"].as_array().unwrap().len(), 4);
        assert_eq!(lib.rt["moves"][0]["id"], json!(2));
        assert_eq!(lib.rt["moves"][1]["id"], json!(4));
        assert_eq!(lib.rt["moves"][1]["address"], json!(256 + 16));
        assert_eq!(lib.rt["descriptor_offset"], json!(0x140));
        assert_eq!(lib.local_memory_bytes, 96);
    }
}

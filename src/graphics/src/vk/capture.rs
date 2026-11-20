//! Frame capture and its replay without the game (mkw-recomp `runtime/gfx/cap`, MIT, reimplemented on this back end).
//!
//! `NXGFX_CAPTURE=<file>` keeps, from the boot, what the GPU side was told that later work depends on (memory pools,
//! texture and sampler registrations, descriptor pools). At present `NXGFX_CAPTURE_AT` (default 600) it writes guest
//! memory (every committed page of the guest's heap and alias regions) and that state, then every submission (its
//! command records, flattened), texture release and present for `NXGFX_CAPTURE_FRAMES` presents (default 2), each
//! submission and present preceded by the guest pages that changed since the last record (4 KiB page hashes, several
//! threads). One zstd stream; the run goes on afterwards.
//!
//! `nxgfx-replay <file> --shaders <dir> [--out <dir>]` maps the regions at the same addresses (guest addresses are
//! host addresses in every process: the records' pointers stay valid), restores them and the state, and feeds the
//! submissions to the back end again; each present is read back to `<out>/frame-<n>.png`.
//!
//! Not in a capture (as mkw-recomp writes them): what only the GPU held when it started (render targets drawn before,
//! device-local pools the GPU wrote), so the first captured frame can differ where it reads earlier frames' results;
//! the frames after it are made from the capture alone. Sparse textures' mappings are not recorded either.

use crate::nvn::obj::{pool_t, samp_par, tex_t, view_t};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;

const MAGIC: u64 = 0x3150_4143_5847_584e; // "NXGXCAP1"
const VERSION: u32 = 1;

/// Record types: {u32 type, u32 0, u64 size} + payload.
pub mod rec {
    pub const META: u32 = 1; // {magic, version, first present, frames}
    pub const REGION: u32 = 2; // {base, size}: a guest allocation to map
    pub const MEM: u32 = 3; // {addr} + bytes
    pub const POOL_ADD: u32 = 4; // {pool_t address}
    pub const POOL_DEL: u32 = 5; // {pool base}
    pub const TEXREG: u32 = 6; // {id, as_image, has_view} + tex_t + view_t
    pub const SAMPREG: u32 = 7; // {id} + samp_par
    pub const TEXREL: u32 = 8; // {serial}
    pub const SUBMIT: u32 = 9; // {queue, origin lower-left} + records
    pub const PRESENT: u32 = 10; // {queue, origin lower-left, has_crop, crop[4]} + tex_t
    pub const DPOOL: u32 = 11; // {tex, pool object address, base}
    pub const END: u32 = 12;
}

const PAGE: u64 = 0x1000;
/// The guest's heap and alias regions (runtime mem.rs: searched from 4 GiB) and its modules (from 0x70_0000_0000: the
/// game's own data holds NVN objects too, memory pools among them), below the kernel region.
const GUEST_LO: u64 = 0x1_0000_0000;
const GUEST_HI: u64 = 0x7F_0000_0000;

#[derive(Default)]
struct Boot {
    pools: Vec<u64>,
    tex: HashMap<i32, (tex_t, Option<view_t>, bool)>,
    samp: HashMap<i32, samp_par>,
}

struct Live {
    out: zstd::stream::write::Encoder<'static, std::io::BufWriter<std::fs::File>>,
    hashes: HashMap<u64, u64>,
    frames_left: u64,
    bytes: u64,
}

#[derive(Default)]
struct St {
    boot: Boot,
    live: Option<Live>,
    presents: u64,
    done: bool,
}

static ST: Mutex<Option<St>> = Mutex::new(None);

/// Capturing is asked for (NXGFX_CAPTURE): the boot state is kept.
pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_CAPTURE").is_ok_and(|v| !v.is_empty()))
}

fn with(f: impl FnOnce(&mut St)) {
    let mut g = ST.lock().unwrap();
    f(g.get_or_insert_with(St::default));
}

fn bytes_of<T>(v: &T) -> &[u8] {
    // SAFETY: plain #[repr(C)] data
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }
}

fn put(l: &mut Live, ty: u32, parts: &[&[u8]]) {
    let n: u64 = parts.iter().map(|p| p.len() as u64).sum();
    let mut h = [0u8; 16];
    h[..4].copy_from_slice(&ty.to_le_bytes());
    h[8..].copy_from_slice(&n.to_le_bytes());
    let _ = l.out.write_all(&h);
    for p in parts {
        let _ = l.out.write_all(p);
    }
    l.bytes += 16 + n;
}

// ---------- hooks (the back end calls them; each does nothing unless NXGFX_CAPTURE) ----------

pub fn pool_add(p: *const pool_t) {
    if !on() {
        return;
    }
    with(|s| {
        let a = p as u64;
        if !s.boot.pools.contains(&a) {
            s.boot.pools.push(a);
        }
        if let Some(l) = s.live.as_mut() {
            put(l, rec::POOL_ADD, &[&a.to_le_bytes()]);
        }
    });
}

pub fn pool_del(base: u64) {
    if !on() {
        return;
    }
    with(|s| {
        // the pool object whose storage starts at base
        s.boot.pools.retain(|&a| unsafe { (*(a as *const pool_t)).mem as u64 } != base);
        if let Some(l) = s.live.as_mut() {
            put(l, rec::POOL_DEL, &[&base.to_le_bytes()]);
        }
    });
}

pub fn tex_reg(id: i32, t: &tex_t, v: Option<view_t>, as_image: bool) {
    if !on() {
        return;
    }
    with(|s| {
        s.boot.tex.insert(id, (*t, v, as_image));
        if let Some(l) = s.live.as_mut() {
            put_texreg(l, id, t, v, as_image);
        }
    });
}

fn put_texreg(l: &mut Live, id: i32, t: &tex_t, v: Option<view_t>, as_image: bool) {
    let h = [id as u32, as_image as u32, v.is_some() as u32, 0];
    let vv = v.unwrap_or_default();
    put(l, rec::TEXREG, &[bytes_of(&h), bytes_of(t), bytes_of(&vv)]);
}

pub fn samp_reg(id: i32, p: &samp_par) {
    if !on() {
        return;
    }
    with(|s| {
        s.boot.samp.insert(id, *p);
        if let Some(l) = s.live.as_mut() {
            put(l, rec::SAMPREG, &[&id.to_le_bytes(), &[0u8; 4], bytes_of(p)]);
        }
    });
}

pub fn tex_release(serial: u64) {
    if !on() {
        return;
    }
    with(|s| {
        s.boot.tex.retain(|_, (t, _, _)| t.serial != serial);
        if let Some(l) = s.live.as_mut() {
            put(l, rec::TEXREL, &[&serial.to_le_bytes()]);
        }
    });
}

/// A submission (its records flattened), before it is queued.
pub fn submit(queue: u64, origin_ll: bool, stream: &[u8]) {
    if !on() {
        return;
    }
    with(|s| {
        if let Some(l) = s.live.as_mut() {
            mem_delta(l);
            put(l, rec::SUBMIT, &[&queue.to_le_bytes(), &(origin_ll as u64).to_le_bytes(), stream]);
        }
    });
}

/// A present, before it is queued: starts the capture at NXGFX_CAPTURE_AT, ends it NXGFX_CAPTURE_FRAMES later.
pub fn present(queue: u64, origin_ll: bool, t: &tex_t, crop: Option<[i32; 4]>) {
    if !on() {
        return;
    }
    let mut g = ST.lock().unwrap();
    let s = g.get_or_insert_with(St::default);
    s.presents += 1;
    if let Some(l) = s.live.as_mut() {
        mem_delta(l);
        let c = crop.unwrap_or([0; 4]);
        let h: [u64; 2] = [queue, origin_ll as u64];
        let hc: [i32; 5] = [crop.is_some() as i32, c[0], c[1], c[2], c[3]];
        put(l, rec::PRESENT, &[bytes_of(&h), bytes_of(&hc), &[0u8; 4], bytes_of(t)]);
        l.frames_left -= 1;
        if l.frames_left == 0 {
            let mut l = s.live.take().unwrap();
            put(&mut l, rec::END, &[]);
            let bytes = l.bytes;
            let _ = l.out.finish().map(|mut w| w.flush());
            s.done = true;
            crate::vklog!("capture: written ({} MB before compression)", bytes >> 20);
        }
        return;
    }
    let at = std::env::var("NXGFX_CAPTURE_AT").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(600);
    if s.done || s.presents < at {
        return;
    }
    let path = std::env::var("NXGFX_CAPTURE").unwrap_or_default();
    let frames = std::env::var("NXGFX_CAPTURE_FRAMES").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(2).max(1);
    // what was submitted before is on the GPU side already
    drop(g);
    super::queue::finish();
    let mut g = ST.lock().unwrap();
    let s = g.get_or_insert_with(St::default);
    let f = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            crate::log::red(&format!("capture: {path}: {e}"));
            s.done = true;
            return;
        }
    };
    let Ok(out) = zstd::stream::write::Encoder::new(std::io::BufWriter::new(f), 3) else { return };
    let mut l = Live { out, hashes: HashMap::new(), frames_left: frames, bytes: 0 };
    let t0 = std::time::Instant::now();
    put(&mut l, rec::META, &[&MAGIC.to_le_bytes(), &VERSION.to_le_bytes(), &[0u8; 4], &s.presents.to_le_bytes(), &frames.to_le_bytes()]);
    // the guest's allocations, then their bytes
    let regions = guest_regions();
    for &(base, size) in &regions {
        put(&mut l, rec::REGION, &[&base.to_le_bytes(), &size.to_le_bytes()]);
    }
    for &(lo, hi) in &committed(&regions) {
        let mut a = lo;
        while a < hi {
            let n = (hi - a).min(16 << 20);
            // SAFETY: committed, readable guest memory (VirtualQuery)
            let b = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
            put(&mut l, rec::MEM, &[&a.to_le_bytes(), b]);
            a += n;
        }
    }
    mem_delta(&mut l); // the page hashes as written (nothing changed meanwhile: the game is in its present)
    for tex in [true, false] {
        for (pool, base) in crate::nvn::pool::dpool_list(tex) {
            put(&mut l, rec::DPOOL, &[&(tex as u64).to_le_bytes(), &(pool as u64).to_le_bytes(), &(base as i64).to_le_bytes()]);
        }
    }
    for &a in &s.boot.pools {
        put(&mut l, rec::POOL_ADD, &[&a.to_le_bytes()]);
    }
    for (&id, &(t, v, img)) in &s.boot.tex {
        put_texreg(&mut l, id, &t, v, img);
    }
    for (&id, p) in &s.boot.samp {
        put(&mut l, rec::SAMPREG, &[&id.to_le_bytes(), &[0u8; 4], bytes_of(p)]);
    }
    crate::vklog!(
        "capture: started at present {} into {path} ({} regions, {} MB of guest memory, {} pools, {} textures, {} samplers, {:.1} s)",
        s.presents,
        regions.len(),
        l.bytes >> 20,
        s.boot.pools.len(),
        s.boot.tex.len(),
        s.boot.samp.len(),
        t0.elapsed().as_secs_f64()
    );
    s.live = Some(l);
}

// ---------- guest memory ----------

/// The guest's allocations (allocation base, size), from VirtualQuery over the guest range.
#[cfg(windows)]
fn guest_regions() -> Vec<(u64, u64)> {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_FREE};
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut a = GUEST_LO;
    while a < GUEST_HI {
        let mut mi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(a as *const _, &mut mi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        let (base, size) = (mi.BaseAddress as u64, mi.RegionSize as u64);
        if mi.State != MEM_FREE {
            let ab = mi.AllocationBase as u64;
            match out.last_mut() {
                Some(l) if l.0 == ab => l.1 = base + size - ab,
                _ => out.push((ab, base + size - ab)),
            }
        }
        a = base + size.max(PAGE);
    }
    out
}
#[cfg(not(windows))]
fn guest_regions() -> Vec<(u64, u64)> {
    Vec::new()
}

/// The committed, readable ranges inside `regions`.
#[cfg(windows)]
fn committed(regions: &[(u64, u64)]) -> Vec<(u64, u64)> {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS};
    let mut out = Vec::new();
    for &(lo, size) in regions {
        let mut a = lo;
        while a < lo + size {
            let mut mi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
            if unsafe { VirtualQuery(a as *const _, &mut mi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
                break;
            }
            let (base, n) = (mi.BaseAddress as u64, mi.RegionSize as u64);
            if mi.State == MEM_COMMIT && mi.Protect & (PAGE_GUARD | PAGE_NOACCESS) == 0 && mi.Protect != 0 {
                out.push((base, (base + n).min(lo + size)));
            }
            a = base + n.max(PAGE);
        }
    }
    out
}
#[cfg(not(windows))]
fn committed(_: &[(u64, u64)]) -> Vec<(u64, u64)> {
    Vec::new()
}

fn page_hash(p: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in p.chunks_exact(8) {
        let v = u64::from_le_bytes(c.try_into().unwrap());
        h = (h ^ v).wrapping_mul(0x0100_0000_01b3).rotate_left(29);
    }
    h
}

/// The guest pages written since the last record (hashes on 8 threads), as MEM records of runs of changed pages.
fn mem_delta(l: &mut Live) {
    let ranges = committed(&guest_regions());
    let pages: Vec<u64> = ranges.iter().flat_map(|&(lo, hi)| (lo..hi).step_by(PAGE as usize)).collect();
    let nt = 8usize;
    let chunk = pages.len().div_ceil(nt).max(1);
    let hashes = &l.hashes;
    let changed: Vec<(u64, u64)> = std::thread::scope(|sc| {
        let hs: Vec<_> = pages
            .chunks(chunk)
            .map(|ps| {
                sc.spawn(move || {
                    ps.iter()
                        .filter_map(|&p| {
                            // SAFETY: a committed, readable page (VirtualQuery just now; the game is inside a call to us)
                            let h = page_hash(unsafe { core::slice::from_raw_parts(p as *const u8, PAGE as usize) });
                            (hashes.get(&p) != Some(&h)).then_some((p, h))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut i = 0;
    while i < changed.len() {
        let mut j = i + 1;
        while j < changed.len() && changed[j].0 == changed[j - 1].0 + PAGE && j - i < 4096 {
            j += 1;
        }
        let (a, n) = (changed[i].0, (j - i) as u64 * PAGE);
        if !l.hashes.is_empty() {
            // SAFETY: as above
            let b = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
            put(l, rec::MEM, &[&a.to_le_bytes(), b]);
        }
        i = j;
    }
    for (p, h) in changed {
        l.hashes.insert(p, h);
    }
}

// ---------- replay ----------

/// A capture's records, one at a time.
pub struct Reader {
    r: zstd::stream::read::Decoder<'static, std::io::BufReader<std::fs::File>>,
}

impl Reader {
    pub fn open(path: &std::path::Path) -> std::io::Result<Reader> {
        Ok(Reader { r: zstd::stream::read::Decoder::new(std::fs::File::open(path)?)? })
    }
    /// The next record: (type, payload); None at the end.
    pub fn next(&mut self) -> Option<(u32, Vec<u8>)> {
        let mut h = [0u8; 16];
        self.r.read_exact(&mut h).ok()?;
        let ty = u32::from_le_bytes(h[..4].try_into().unwrap());
        let n = u64::from_le_bytes(h[8..].try_into().unwrap());
        let mut p = vec![0u8; n as usize];
        self.r.read_exact(&mut p).ok()?;
        Some((ty, p))
    }
}

fn u64_at(p: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(p[o..o + 8].try_into().unwrap())
}
fn read_t<T: Copy>(p: &[u8], o: usize) -> T {
    // SAFETY: the record holds a T there (the capture wrote it)
    unsafe { core::ptr::read_unaligned(p[o..o + core::mem::size_of::<T>()].as_ptr() as *const T) }
}

/// Reserves a captured allocation at its own address (its pages are committed as their bytes come: `commit`).
#[cfg(windows)]
fn map_region(base: u64, size: u64) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_RESERVE, PAGE_READWRITE};
    let p = unsafe { VirtualAlloc(base as *const _, size as usize, MEM_RESERVE, PAGE_READWRITE) };
    p as u64 == base
}
#[cfg(not(windows))]
fn map_region(_: u64, _: u64) -> bool {
    false
}
#[cfg(windows)]
fn commit(a: u64, n: u64) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, PAGE_READWRITE};
    !unsafe { VirtualAlloc(a as *const _, n as usize, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}
#[cfg(not(windows))]
fn commit(_: u64, _: u64) -> bool {
    false
}

/// Replays capture `path` on this process's back end (`set_shader_dir` first); writes each present to
/// `<out>/frame-<n>.png`. Returns the presents written.
pub fn replay(path: &std::path::Path, out: &std::path::Path) -> Result<u64, String> {
    let mut r = Reader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (ty, meta) = r.next().ok_or("an empty capture")?;
    if ty != rec::META || u64_at(&meta, 0) != MAGIC {
        return Err("not a capture of this format".into());
    }
    let g = super::Gpu::get()?;
    let _ = std::fs::create_dir_all(out);
    let (mut frames, mut subs, mut failed_regions) = (0u64, 0u64, 0u64);
    let trace = std::env::var("NXGFX_REPLAY_TRACE").is_ok();
    let mut n_rec = 0u64;
    while let Some((ty, p)) = r.next() {
        n_rec += 1;
        if trace {
            eprintln!("replay: record {n_rec} type {ty} ({} bytes){}", p.len(), if p.len() >= 8 { format!(" first word {:#x}", u64_at(&p, 0)) } else { String::new() });
        }
        match ty {
            rec::REGION => {
                let (b, n) = (u64_at(&p, 0), u64_at(&p, 8));
                if !map_region(b, n) {
                    failed_regions += 1;
                    crate::log::red(&format!("replay: the capture's region {b:#x} +{n:#x} could not be reserved at its address (this process holds memory there)"));
                }
            }
            rec::MEM => {
                let a = u64_at(&p, 0);
                if !commit(a, (p.len() - 8) as u64) {
                    failed_regions += 1;
                    continue;
                }
                // SAFETY: committed just now, inside a region reserved above (a page the capture read)
                unsafe { core::ptr::copy_nonoverlapping(p[8..].as_ptr(), a as *mut u8, p.len() - 8) };
            }
            rec::DPOOL if failed_regions > 0 => return Err(format!("{failed_regions} of the capture's memory regions could not be placed at their addresses")),
            rec::DPOOL => {
                let (tex, pool, base) = (u64_at(&p, 0) != 0, u64_at(&p, 8) as usize, u64_at(&p, 16) as i64 as i32);
                let n = unsafe { (*(pool as *const crate::nvn::obj::dpool_t)).n };
                crate::nvn::pool::dpool_restore(tex, pool, base, n);
                if tex {
                    g.tables.reserve_tex(base, n);
                } else {
                    g.tables.reserve_samp(base, n);
                }
            }
            rec::POOL_ADD => super::mem::pool_add(unsafe { &*(u64_at(&p, 0) as *const pool_t) }),
            rec::POOL_DEL => super::mem::pool_del(u64_at(&p, 0)),
            rec::TEXREG => {
                let id = u32::from_le_bytes(p[0..4].try_into().unwrap()) as i32;
                let (img, hv) = (p[4] != 0, p[8] != 0);
                let t: tex_t = read_t(&p, 16);
                let v: view_t = read_t(&p, 16 + core::mem::size_of::<tex_t>());
                g.reg_tex(id, &t, hv.then_some(v), img);
            }
            rec::SAMPREG => {
                let id = u32::from_le_bytes(p[0..4].try_into().unwrap()) as i32;
                let s: samp_par = read_t(&p, 8);
                g.reg_samp(id, &s);
            }
            rec::TEXREL => g.tex_release(u64_at(&p, 0)),
            rec::SUBMIT => {
                super::queue::submit_stream(u64_at(&p, 0), u64_at(&p, 8) != 0, p[16..].to_vec());
                subs += 1;
            }
            rec::PRESENT => {
                let ll = u64_at(&p, 8) != 0;
                let hc: [i32; 5] = read_t(&p, 16);
                let t: tex_t = read_t(&p, 40);
                super::queue::finish();
                let crop = (hc[0] != 0).then_some([hc[1], hc[2], hc[3], hc[4]]);
                if let Some((rgba, w, h)) = super::shot::capture(&t, crop, ll) {
                    let f = out.join(format!("frame-{frames}.png"));
                    super::shot::png_write(&f, &rgba, w, h).map_err(|e| format!("{}: {e}", f.display()))?;
                }
                frames += 1;
            }
            rec::END => break,
            _ => {}
        }
    }
    super::queue::finish();
    crate::vklog!("replay: {frames} presents, {subs} submissions{}", if failed_regions > 0 { format!("; {failed_regions} regions could not be mapped at their address") } else { String::new() });
    Ok(frames)
}

#[cfg(test)]
mod tests {
    #[test]
    fn records_read_back_in_order() {
        let path = std::env::temp_dir().join(format!("nxgfx-cap-test-{}.cap", std::process::id()));
        {
            let f = std::fs::File::create(&path).unwrap();
            let out = zstd::stream::write::Encoder::new(std::io::BufWriter::new(f), 3).unwrap();
            let mut l = super::Live { out, hashes: Default::default(), frames_left: 1, bytes: 0 };
            super::put(&mut l, super::rec::META, &[&super::MAGIC.to_le_bytes()]);
            super::put(&mut l, super::rec::SUBMIT, &[&7u64.to_le_bytes(), &1u64.to_le_bytes(), &[1, 2, 3]]);
            super::put(&mut l, super::rec::END, &[]);
            use std::io::Write;
            l.out.finish().unwrap().flush().unwrap();
        }
        let mut r = super::Reader::open(&path).unwrap();
        assert_eq!(r.next().map(|x| x.0), Some(super::rec::META));
        let (t, p) = r.next().unwrap();
        assert_eq!((t, super::u64_at(&p, 0), &p[16..]), (super::rec::SUBMIT, 7, &[1u8, 2, 3][..]));
        assert_eq!(r.next().map(|x| x.0), Some(super::rec::END));
        assert!(r.next().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn page_hash_sees_one_byte() {
        let mut a = vec![0u8; 4096];
        let h0 = super::page_hash(&a);
        a[4095] = 1;
        assert_ne!(h0, super::page_hash(&a));
    }
}

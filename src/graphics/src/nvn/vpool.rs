//! Virtual memory pools (NVN `VIRTUAL`, flags 0x200): a GPU address range with no storage of its own;
//! `nvnMemoryPoolMapVirtual` maps pages of physical pools into it. A mapping request is 0x28 bytes: {physical pool,
//! physical offset, virtual offset, size, storage class}.
//!
//! Every virtual pool gets an address range of its own at Initialize (reserved, inaccessible), since a game may create
//! its textures and buffers before mapping anything. The first mapping request decides what the pool is:
//!
//! - **Alias**: a request that maps the whole pool from offset 0 onto one physical pool. The reservation is given back
//!   and the pool's storage is that physical memory: the same pages on the console's one page table, so buffer and
//!   texture addresses land in the physical pool the replay already knows. Later requests must keep that one base.
//! - **Sparse**: anything else, and a pool larger than the application pool from the start. Every request goes into a
//!   page table at 4 KB (the physical memory behind each virtual page and the mapping generation that set it). Windows
//!   cannot map pages of private memory at a second address, so the CPU cannot address the pool's range; the GPU
//!   does through the table: a buffer range whose pages are contiguous physical memory resolves to that memory
//!   (`translate`), and a texture is uploaded from what the table points at, unmapped pages read as 0 (`copy`), again
//!   when a mapping in its range changes (`gen_in`) or the CPU writes a physical page behind it.

use super::obj::pool_t;
use super::types::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

const LEAF: usize = 512; // a leaf covers 2 MB of 4 KB pages
const PG: u64 = 0x1000;

#[derive(Clone, Copy, Default)]
struct Pte {
    phys: u64,
    gen: u64,
}

struct VPool {
    pool: usize,
    base: u64,
    len: u64,
    size: u64,
    top: u64,
    pt: Vec<Option<Box<[Pte; LEAF]>>>,
}

static POOLS: RwLock<Vec<VPool>> = RwLock::new(Vec::new());
static GEN: AtomicU64 = AtomicU64::new(0);
/// The title's application pool (bytes): a virtual pool larger than it cannot be one alias of a physical pool.
pub static APP_POOL: AtomicU64 = AtomicU64::new(3 << 30);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MapReq {
    pub phys: *const pool_t,
    pub poff: i64,
    pub voff: i64,
    pub size: u64,
    pub storage_class: i32,
    pub pad: i32,
}
const _: () = assert!(core::mem::size_of::<MapReq>() == 0x28);

#[cfg(windows)]
fn reserve(len: u64) -> u64 {
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_RESERVE, PAGE_NOACCESS};
    unsafe { VirtualAlloc(core::ptr::null(), len as usize, MEM_RESERVE, PAGE_NOACCESS) as u64 }
}
#[cfg(windows)]
fn unreserve(a: u64) {
    use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};
    unsafe { VirtualFree(a as *mut _, 0, MEM_RELEASE) };
}
#[cfg(not(windows))]
fn reserve(_len: u64) -> u64 {
    0
}
#[cfg(not(windows))]
fn unreserve(_a: u64) {}

/// vmode: 0 undecided, 1 alias, 2 sparse.
pub const UNDECIDED: i32 = 0;
pub const ALIAS: i32 = 1;
pub const SPARSE: i32 = 2;

/// nvnMemoryPoolInitialize of a virtual pool: its own address range (the pool's storage address until a mapping
/// decides). Returns the range's base, 0 when the address space cannot be reserved.
pub fn initialize(o: &mut pool_t) -> u64 {
    let len = (o.size.min(64 << 30) + 0xFFFF) & !0xFFFF;
    let a = reserve(len);
    if a == 0 {
        crate::log::red(&format!("virtual pool {:p}: cannot reserve {len:#x} bytes of address space", o as *const _));
        return 0;
    }
    let leaves = o.size.div_ceil(LEAF as u64 * PG) as usize;
    o.vmode = if o.size > APP_POOL.load(Ordering::Relaxed) { SPARSE } else { UNDECIDED };
    if o.vmode == SPARSE {
        crate::nvnlog!("virtual pool {:p} ({:#x} bytes, flags {:#x}): sparse, own address range {a:#x}+{len:#x}, kept in a page table", o as *const _, o.size, o.flags);
    }
    let mut g = POOLS.write().unwrap();
    g.push(VPool { pool: o as *const _ as usize, base: a, len, size: o.size, top: 0, pt: (0..leaves).map(|_| None).collect() });
    a
}

fn drop_range(pool: usize) {
    let mut g = POOLS.write().unwrap();
    if let Some(i) = g.iter().position(|v| v.pool == pool) {
        let v = g.swap_remove(i);
        unreserve(v.base);
    }
}

/// nvnMemoryPoolFinalize of a virtual pool.
pub fn finalize(o: &mut pool_t) {
    drop_range(o as *const _ as usize);
}

/// nvnMemoryPoolMapVirtual.
pub fn map(o: &mut pool_t, rq: &[MapReq]) -> bool {
    if o.vmode == UNDECIDED && !rq.is_empty() {
        let r = rq[0];
        if !r.phys.is_null() && r.voff == 0 && r.size == o.size {
            drop_range(o as *const _ as usize);
            o.mem = core::ptr::null_mut();
            o.vmode = ALIAS;
        } else {
            o.vmode = SPARSE;
            crate::nvnlog!("virtual pool {:p} ({:#x} bytes): its first mapping is {:#x}+{:#x}: sparse, kept in a page table", o as *const _, o.size, r.voff, r.size);
        }
    }
    if o.vmode == SPARSE {
        let mut g = POOLS.write().unwrap();
        let Some(v) = g.iter_mut().find(|v| v.pool == o as *const _ as usize) else {
            crate::log::red_once("MapVirtual: sparse pool without its address range");
            return false;
        };
        let base = v.base;
        let ok = rq.iter().all(|r| map_sparse(v, r));
        drop(g);
        // sparse images over the remapped ranges bind their blocks again
        for r in rq {
            if r.voff >= 0 {
                crate::vk::sparse::remap(base + r.voff as u64, r.size);
            }
        }
        return ok;
    }
    for r in rq {
        if r.phys.is_null() {
            continue; // unmapping an alias: the storage stays as is
        }
        // SAFETY: the game's physical pool object
        let ph = unsafe { &*r.phys };
        if ph.mem.is_null() {
            crate::log::red(&format!("MapVirtual: physical pool {:p} has no storage", r.phys));
            return false;
        }
        if r.voff < 0 || r.voff as u64 + r.size > o.size || r.poff < 0 || r.poff as u64 + r.size > ph.size {
            crate::log::red(&format!("MapVirtual: request out of range (virtual {:#x}+{:#x} / pool {:#x}, physical {:#x} / pool {:#x})", r.voff, r.size, o.size, r.poff, ph.size));
            return false;
        }
        let base = (ph.mem as i64 + r.poff - r.voff) as *mut u8;
        if !o.mem.is_null() && o.mem != base {
            // the pool was an alias of one base: its objects' addresses are in that physical pool, so a second base
            // cannot be served (the console's game would have needed a sparse pool from its first request)
            crate::log::red(&format!("MapVirtual: virtual pool {:p} is an alias of {:p}; a request for a second base {:p} is refused", o as *const _, o.mem, base));
            return false;
        }
        if o.mem.is_null() {
            crate::nvnlog!("MapVirtual: virtual pool {:p} ({:#x} bytes) is physical pool {:p} + {:#x}", o as *const _, o.size, r.phys, r.poff - r.voff);
        }
        o.mem = base;
    }
    true
}

fn set(v: &mut VPool, voff: u64, size: u64, phys: Option<u64>) {
    let gen = GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let mut o = 0;
    while o < size {
        let at = voff + o;
        let li = (at / (LEAF as u64 * PG)) as usize;
        if li >= v.pt.len() {
            break;
        }
        if v.pt[li].is_none() {
            if phys.is_none() {
                o += PG;
                continue;
            }
            v.pt[li] = Some(Box::new([Pte::default(); LEAF]));
        }
        let leaf = v.pt[li].as_mut().unwrap();
        leaf[((at / PG) as usize) % LEAF] = Pte { phys: phys.map(|p| p + o).unwrap_or(0), gen };
        o += PG;
    }
}

fn map_sparse(v: &mut VPool, r: &MapReq) -> bool {
    if r.voff < 0 || r.voff as u64 + r.size > v.size {
        crate::log::red(&format!("virtual pool {:#x}: request at {:#x}+{:#x} is beyond the pool's {:#x}", v.pool, r.voff, r.size, v.size));
        return false;
    }
    if (r.voff as u64 | r.size | r.poff as u64) & (PG - 1) != 0 {
        crate::log::red(&format!("virtual pool {:#x}: request {:#x}+{:#x} (physical offset {:#x}) is not 4 KB aligned", v.pool, r.voff, r.size, r.poff));
        return false;
    }
    let phys = if r.phys.is_null() {
        None
    } else {
        // SAFETY: the game's physical pool object
        let ph = unsafe { &*r.phys };
        if ph.mem.is_null() || r.poff < 0 || r.poff as u64 + r.size > ph.size {
            crate::log::red(&format!("virtual pool {:#x}: physical pool {:p} ({:#x} bytes) cannot supply {:#x}+{:#x}", v.pool, r.phys, ph.size, r.poff, r.size));
            return false;
        }
        Some(ph.mem as u64 + r.poff as u64)
    };
    set(v, r.voff as u64, r.size, phys);
    let end = r.voff as u64 + r.size;
    if phys.is_some() && end >> 27 > v.top >> 27 {
        crate::nvnlog!("virtual pool {:#x}: mapped up to {end:#x} (page table)", v.pool);
    }
    if phys.is_some() && end > v.top {
        v.top = end;
    }
    true
}

fn pte(v: &VPool, off: u64) -> Pte {
    v.pt.get((off / (LEAF as u64 * PG)) as usize).and_then(|l| l.as_ref()).map(|l| l[((off / PG) as usize) % LEAF]).unwrap_or_default()
}

fn with_pool<R>(a: u64, f: impl FnOnce(&VPool, u64) -> R) -> Option<R> {
    let g = POOLS.read().unwrap();
    let v = g.iter().find(|v| a >= v.base && a < v.base + v.len.max(v.size))?;
    Some(f(v, a - v.base))
}

/// Whether the sparse virtual pool holding `a` is one the CPU cannot access (its textures only the GPU writes).
pub fn gpu_only(a: u64) -> bool {
    // SAFETY: the game's pool object, live while its range is registered
    with_pool(a, |v, _| unsafe { (*(v.pool as *const pool_t)).flags } & mem_flags::CPU_NO_ACCESS != 0).unwrap_or(false)
}

/// The virtual address range [base, base + size) a sparse pool's offsets map into.
pub fn range_of(pool: usize) -> Option<(u64, u64)> {
    POOLS.read().unwrap().iter().find(|v| v.pool == pool).map(|v| (v.base, v.size))
}

/// Whether `a` is in a sparse virtual pool's range.
pub fn is_sparse(a: u64) -> bool {
    with_pool(a, |_, _| ()).is_some()
}

/// The physical address behind [a, a + n) when every page of it is mapped and the pages are contiguous.
pub fn translate(a: u64, n: u64) -> Option<u64> {
    with_pool(a, |v, off| {
        let first = pte(v, off);
        if first.phys == 0 {
            return None;
        }
        let start = first.phys + (off & (PG - 1));
        let mut o = (off & !(PG - 1)) + PG;
        while o < off + n.max(1) {
            let p = pte(v, o);
            if p.phys == 0 || p.phys != start - (off & (PG - 1)) + (o - (off & !(PG - 1))) {
                return None;
            }
            o += PG;
        }
        Some(start)
    })
    .flatten()
}

/// Reads [a, a + dst.len()) through the page table: unmapped pages read as 0. Returns the physical ranges read
/// (for dirty tracking) or None when `a` is in no sparse pool.
pub fn copy(a: u64, dst: &mut [u8]) -> Option<Vec<(u64, u64)>> {
    with_pool(a, |v, off| {
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut i = 0usize;
        while i < dst.len() {
            let o = off + i as u64;
            let in_pg = (PG - (o & (PG - 1))) as usize;
            let n = in_pg.min(dst.len() - i);
            let p = pte(v, o);
            if p.phys == 0 {
                dst[i..i + n].fill(0);
            } else {
                let src = p.phys + (o & (PG - 1));
                // SAFETY: mapped physical pool memory (the game's own pages)
                unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst.as_mut_ptr().add(i), n) };
                match runs.last_mut() {
                    Some(l) if l.0 + l.1 == src => l.1 += n as u64,
                    _ => runs.push((src, n as u64)),
                }
            }
            i += n;
        }
        runs
    })
}

/// The newest mapping generation over [a, a + n) (0: never mapped).
pub fn gen_in(a: u64, n: u64) -> u64 {
    with_pool(a, |v, off| {
        let mut g = 0;
        let mut o = off & !(PG - 1);
        while o < off + n {
            g = g.max(pte(v, o).gen);
            o += PG;
        }
        g
    })
    .unwrap_or(0)
}

/// Every mapping request so far (a texture compares it to skip `gen_in` scans when nothing changed).
pub fn generation() -> u64 {
    GEN.load(Ordering::Relaxed)
}

pub fn is_virtual(p: &pool_t) -> bool {
    p.flags & mem_flags::VIRTUAL != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pool(size: u64, mem: *mut u8, flags: u32) -> pool_t {
        pool_t { magic: 0, flags, mem, size, vp: core::ptr::null_mut(), vmode: 0, serial: 0 }
    }
    #[test]
    fn alias_then_sparse_page_table() {
        let mut phys_mem = vec![0u8; 0x10000];
        for (i, b) in phys_mem.iter_mut().enumerate() {
            *b = (i >> 12) as u8 + 1;
        }
        let ph = pool(0x10000, phys_mem.as_mut_ptr(), 0);
        // whole-pool mapping: alias
        let mut v = pool(0x8000, core::ptr::null_mut(), mem_flags::VIRTUAL);
        v.mem = initialize(&mut v) as *mut u8;
        assert!(!v.mem.is_null());
        assert!(map(&mut v, &[MapReq { phys: &ph, poff: 0x4000, voff: 0, size: 0x8000, storage_class: 0, pad: 0 }]));
        assert_eq!(v.vmode, ALIAS);
        assert_eq!(v.mem as u64, phys_mem.as_ptr() as u64 + 0x4000);
        // partial mapping: sparse
        let mut s = pool(0x8000, core::ptr::null_mut(), mem_flags::VIRTUAL);
        s.mem = initialize(&mut s) as *mut u8;
        let base = s.mem as u64;
        assert!(map(&mut s, &[MapReq { phys: &ph, poff: 0x3000, voff: 0x1000, size: 0x2000, storage_class: 0, pad: 0 }]));
        assert_eq!(s.vmode, SPARSE);
        assert_eq!(translate(base + 0x1010, 0x1ff0), Some(phys_mem.as_ptr() as u64 + 0x3010));
        assert_eq!(translate(base, 0x10), None);
        let mut buf = vec![0xffu8; 0x3000];
        let runs = copy(base, &mut buf).unwrap();
        assert_eq!(buf[0], 0);
        assert_eq!(buf[0x1000], 4);
        assert_eq!(buf[0x2000], 5);
        assert_eq!(runs, vec![(phys_mem.as_ptr() as u64 + 0x3000, 0x2000)]);
        assert!(gen_in(base, 0x3000) > 0);
        finalize(&mut s);
        assert!(!is_sparse(base));
    }
}

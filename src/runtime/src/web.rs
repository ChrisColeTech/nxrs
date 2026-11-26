//! Guest memory on the web (docs/35): one wasm64 linear memory, and guest addresses are linear-memory addresses (the
//! web build translates the modules at `WEB_FIRST_BASE`, and the compact map keeps every guest region the guest reads
//! below the linear memory's limit).
//!
//! A reservation claims the linear memory under it from the heap's break (`sbrk`), so the allocator never hands it
//! out. Commit, decommit and protection are bookkeeping in a page table: wasm has no page protection, so a page marked
//! read-only or no-access is still writable by the CPU; `readable` answers from the table. Decommitted pages are
//! zeroed (they come back zeroed, as on Windows). There is no write watch (docs/35 W4 replaces its one user).
//! A range beyond the linear memory (the host-function window, whose addresses are only ever called, never read) is
//! recorded without backing.

use std::collections::BTreeMap;
use std::sync::Mutex;

const PAGE: u64 = 0x1000;
/// The linear memory's limit: the web link's `-sMAXIMUM_MEMORY=16GB` (Chrome's cap for 64-bit memories).
pub const LINEAR_LIMIT: u64 = 16 << 30;
const PAGES: usize = (LINEAR_LIMIT / PAGE) as usize;

extern "C" {
    fn sbrk(increment: isize) -> *mut core::ffi::c_void;
}

/// Page states in the table.
const P_FREE: u8 = 0;
const P_NONE: u8 = 1;
const P_READ: u8 = 2;
const P_RW: u8 = 3;

struct State {
    /// Reservations by base: (size, backed by linear memory).
    res: BTreeMap<u64, (u64, bool)>,
    /// Linear memory claimed from the break for reservations: [lo, hi) runs, in order.
    claimed: Vec<(u64, u64)>,
    /// One byte per 4 KiB page of the linear memory (P_*).
    pages: Vec<u8>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let s = g.get_or_insert_with(|| State {
        res: BTreeMap::new(),
        claimed: vec![],
        pages: vec![P_FREE; PAGES],
    });
    f(s)
}

fn overlaps(s: &State, addr: u64, n: u64) -> bool {
    s.res
        .range(..addr.saturating_add(n))
        .next_back()
        .is_some_and(|(&b, &(z, _))| b + z > addr)
}

fn owned(s: &State, addr: u64, n: u64) -> bool {
    s.claimed.iter().any(|&(lo, hi)| addr >= lo && addr + n <= hi)
}

/// Reserves [addr, addr+n) exactly there. Below the linear memory's limit the memory is claimed from the heap's break;
/// a range under memory the heap already uses cannot be reserved.
pub fn reserve(addr: u64, n: u64, _watch: bool) -> bool {
    with(|s| {
        if n == 0 || overlaps(s, addr, n) {
            return false;
        }
        if addr.saturating_add(n) > LINEAR_LIMIT {
            s.res.insert(addr, (n, false));
            return true;
        }
        if !owned(s, addr, n) {
            // SAFETY: sbrk(0) reads the break
            let brk = unsafe { sbrk(0) } as u64;
            if brk > addr {
                return false;
            }
            let grow = (addr + n - brk) as isize;
            // SAFETY: moves the break up to the reservation's end; the memory between is ours from now on
            if unsafe { sbrk(grow) } as isize == -1 {
                return false;
            }
            s.claimed.push((brk, addr + n));
        }
        s.res.insert(addr, (n, true));
        true
    })
}

/// Reserves n bytes wherever there is room: claimed from the break, page-aligned.
pub fn reserve_anywhere(n: u64, watch: bool) -> Option<u64> {
    // SAFETY: sbrk(0) reads the break
    let brk = unsafe { sbrk(0) } as u64;
    let at = (brk + PAGE - 1) & !(PAGE - 1);
    reserve(at, (n + PAGE - 1) & !(PAGE - 1), watch).then_some(at)
}

pub fn release(addr: u64) {
    with(|s| {
        if let Some((n, true)) = s.res.remove(&addr) {
            set(s, addr, n, P_FREE);
        }
    })
}

fn set(s: &mut State, addr: u64, n: u64, v: u8) {
    let (a, b) = ((addr / PAGE) as usize, (addr + n).div_ceil(PAGE) as usize);
    s.pages[a.min(PAGES)..b.min(PAGES)].fill(v);
}

fn backed(s: &State, addr: u64, n: u64) -> bool {
    s.res
        .range(..=addr)
        .next_back()
        .is_some_and(|(&b, &(z, backed))| backed && addr + n <= b + z)
}

pub fn commit(addr: u64, n: u64) -> bool {
    n == 0
        || with(|s| {
            if !backed(s, addr, n) {
                return false;
            }
            let (a, b) = ((addr / PAGE) as usize, (addr + n).div_ceil(PAGE) as usize);
            for p in &mut s.pages[a..b] {
                if *p == P_FREE {
                    *p = P_RW;
                }
            }
            true
        })
}

pub fn decommit(addr: u64, n: u64) -> bool {
    n == 0
        || with(|s| {
            if !backed(s, addr, n) {
                return false;
            }
            // SAFETY: inside a backed reservation of the linear memory
            unsafe { core::ptr::write_bytes(addr as *mut u8, 0, n as usize) };
            set(s, addr, n, P_FREE);
            true
        })
}

/// 0 no access, 1 read, 2 read-write (as `linux::protect`). Recorded only: wasm cannot protect a page.
pub fn protect(addr: u64, n: u64, prot: u8) -> bool {
    n == 0
        || with(|s| {
            let v = [P_NONE, P_READ, P_RW][prot.min(2) as usize];
            let (a, b) = ((addr / PAGE) as usize, (addr + n).div_ceil(PAGE) as usize);
            if b > PAGES {
                return false;
            }
            for p in &mut s.pages[a..b] {
                if *p != P_FREE {
                    *p = v;
                }
            }
            true
        })
}

/// Zeroed host memory for the runtime's own tables.
pub fn host_alloc(n: u64) -> Option<*mut u8> {
    let layout = std::alloc::Layout::from_size_align(n.max(1) as usize, PAGE as usize).ok()?;
    // SAFETY: a non-zero layout; the runtime keeps its tables for the process lifetime
    let p = unsafe { std::alloc::alloc_zeroed(layout) };
    (!p.is_null()).then_some(p)
}

/// Every page of [addr, addr+n) is committed and readable (by the table).
pub fn readable(addr: u64, n: u64) -> bool {
    let end = addr.saturating_add(n.max(1));
    if end > LINEAR_LIMIT {
        return false;
    }
    with(|s| {
        s.pages[(addr / PAGE) as usize..end.div_ceil(PAGE) as usize]
            .iter()
            .all(|&p| p >= P_READ)
    })
}

/// Committed bytes of [a, a+n) (the part below the linear memory's limit).
pub fn committed_in(a: u64, n: u64) -> u64 {
    let end = a.saturating_add(n).min(LINEAR_LIMIT);
    if a >= end {
        return 0;
    }
    with(|s| {
        s.pages[(a / PAGE) as usize..end.div_ceil(PAGE) as usize]
            .iter()
            .filter(|&&p| p != P_FREE)
            .count() as u64
            * PAGE
    })
}

/// What holds the address, for the "taken" errors.
pub fn describe(addr: u64) -> String {
    // SAFETY: sbrk(0) reads the break
    let brk = unsafe { sbrk(0) } as u64;
    with(|s| match s.res.range(..=addr).next_back() {
        Some((&b, &(n, backed))) if addr < b + n => format!(
            " (inside the reservation {b:#x}+{n:#x}{})",
            if backed { "" } else { ", not backed" }
        ),
        _ if addr < brk => format!(" (under the runtime heap, whose break is {brk:#x})"),
        _ => String::new(),
    })
}

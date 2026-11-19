//! The process's memory as the os layer serves it, on the runtime's address space (`nxrt::mem::Space`):
//!
//! * the heap (`SetHeapSize`) is committed lazily: growing only moves its end, and a page is committed on its first
//!   touch, by the guest (a vectored exception handler that runs before the runtime's crash reporter) or by host
//!   code (`prepare`, before an OS call writes guest memory);
//! * `MapMemory` / `UnmapMemory` (the SDK moves a thread's heap stack into the stack region) copy, as one address
//!   space allows: the source is left untouched while mapped, as on Horizon where it is inaccessible;
//! * `MapPhysicalMemory` / `UnmapPhysicalMemory` commit and release pages of the alias region;
//! * transfer memory stays where it is (the only receivers are host services); shared memory a host service owns
//!   is backed at the address the guest maps it to, and the service writes there;
//! * `QueryMemory` reports these regions over the runtime's module, stack and TLS regions, and `GetInfo` answers
//!   the process memory ids from the title's profile (pool, used memory, system resource).

use nxrt::mem::{self as rmem, MemoryInfo, Range, Space};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};

pub const PAGE: u64 = 0x1000;
const LAZY_CHUNK: u64 = 0x1_0000;

pub const R_INVALID_SIZE: u64 = 0xCA01;
pub const R_INVALID_ADDRESS: u64 = 0xCC01;
pub const R_INVALID_STATE: u64 = 0xD401;
pub const R_OUT_OF_MEMORY: u64 = 0xD001;
pub const R_LIMIT_REACHED: u64 = 0x10801;
pub const R_OUT_OF_RESOURCE: u64 = 0xCE01;

pub const ST_FREE: u32 = 0;
pub const ST_NORMAL: u32 = 5;
pub const ST_SHARED: u32 = 6;
pub const ST_STACK: u32 = 0xB;
pub const ATTR_LOCKED: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub dst: u64,
    pub src: u64,
    pub size: u64,
}

#[derive(Default)]
struct State {
    heap_size: u64,
    heap_seen: u64,
    maps: Vec<Mapping>,
    /// Mapped pages of the alias region, as (base, size) runs (MapPhysicalMemory).
    phys: Vec<(u64, u64)>,
    phys_bytes: u64,
    /// Shared memory views: (address, size).
    shared: Vec<(u64, u64)>,
}

/// The process memory figures a title's profile gives.
#[derive(Clone, Copy, Debug)]
pub struct Figures {
    /// The application pool (GetInfo TotalMemorySize).
    pub pool: u64,
    /// Memory in use besides the heap (GetInfo UsedMemorySize is the heap plus this).
    pub used_beside_heap: u64,
    /// The NPDM's system resource (0: no virtual address memory).
    pub sys_resource: u64,
}

pub struct Memory {
    pub space: &'static Space,
    pub fig: Figures,
    st: Mutex<State>,
}

static MEM: OnceLock<Memory> = OnceLock::new();

pub fn init(space: &'static Space, fig: Figures) -> &'static Memory {
    let _ = MEM.set(Memory { space, fig, st: Mutex::new(State::default()) });
    if cfg!(test) {
        install_fault_handler();
    }
    get()
}

/// Installs the first-touch commit handler. Vectored handlers added later run first: this goes in after the
/// runtime's crash reporter, so a heap page's first touch never reaches the reporter.
pub fn install_fault_handler() {
    unsafe { AddVectoredExceptionHandler(1, Some(veh)) };
}

pub fn get() -> &'static Memory {
    MEM.get().expect("os memory used before init")
}

pub fn try_get() -> Option<&'static Memory> {
    MEM.get()
}

fn commit_rw(a: u64, n: u64) -> bool {
    if n == 0 {
        return true;
    }
    if !unsafe { VirtualAlloc(a as _, n as usize, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
        return true;
    }
    // Outside the runtime's reservations: reserve and commit there.
    let p = unsafe { VirtualAlloc(a as _, n as usize, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
    !p.is_null() && p as u64 == a
}

/// Commits the heap pages of [a, a+n) that are inside the heap's current size (a no-op elsewhere): host code calls
/// it before an OS call writes guest memory, since a kernel write to an uncommitted page fails instead of faulting.
pub fn prepare(a: u64, n: u64) {
    let Some(m) = MEM.get() else { return };
    let (hb, hs) = (m.space.heap.base, m.st.lock().unwrap().heap_size);
    let lo = a.max(hb) & !(PAGE - 1);
    let hi = rmem::align_up(a.saturating_add(n).min(hb + hs), PAGE);
    if lo < hi {
        commit_rw(lo, hi - lo);
    }
}

unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let rec = &*(*info).ExceptionRecord;
    if rec.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS || rec.NumberParameters < 2 {
        return CONTINUE_SEARCH;
    }
    let a = rec.ExceptionInformation[1] as u64;
    let Some(m) = MEM.get() else { return CONTINUE_SEARCH };
    let hb = m.space.heap.base;
    if a < hb || a >= hb + m.space.heap.size {
        return CONTINUE_SEARCH;
    }
    let hs = m.st.lock().map(|s| s.heap_size).unwrap_or(0);
    if a >= hb + hs {
        return CONTINUE_SEARCH;
    }
    let lo = (a & !(LAZY_CHUNK - 1)).max(hb);
    let hi = ((a & !(LAZY_CHUNK - 1)) + LAZY_CHUNK).min(hb + hs);
    if commit_rw(lo, hi - lo) {
        CONTINUE_EXECUTION
    } else {
        CONTINUE_SEARCH
    }
}

impl Memory {
    /// svcSetHeapSize: the heap's end moves in 2 MiB steps; pages are committed on first touch.
    pub fn set_heap_size(&self, size: u64) -> Result<u64, u64> {
        if !size.is_multiple_of(0x20_0000) {
            return Err(R_INVALID_SIZE);
        }
        if size > self.space.heap.size {
            crate::log!("SetHeapSize {size:#x}: larger than the heap region {:#x} (ResultOutOfMemory)", self.space.heap.size);
            return Err(R_OUT_OF_MEMORY);
        }
        let mut st = self.st.lock().unwrap();
        let used = self.fig.used_beside_heap + st.phys_bytes;
        if used > self.fig.pool || size > self.fig.pool - used {
            crate::log_once!("heap-limit", "SetHeapSize {size:#x} > pool {:#x} less used {used:#x} (ResultLimitReached)", self.fig.pool);
            return Err(R_LIMIT_REACHED);
        }
        let b = self.space.heap.base;
        if size < st.heap_size {
            if st.maps.iter().any(|m| m.src < b + st.heap_size && m.src + m.size > b + size) {
                return Err(R_INVALID_STATE);
            }
            rmem::decommit(b + size, st.heap_size - size);
        }
        if size > st.heap_size {
            crate::log!("heap {:.2} GB (lazily committed; region {:.2} GB)", size as f64 / 1e9, self.space.heap.size as f64 / 1e9);
        }
        st.heap_size = size;
        st.heap_seen = st.heap_seen.max(size);
        Ok(b)
    }

    pub fn heap_size(&self) -> u64 {
        self.st.lock().unwrap().heap_size
    }

    fn in_heap(&self, st: &State, a: u64, n: u64) -> bool {
        let b = self.space.heap.base;
        a >= b && a + n <= b + st.heap_size
    }

    /// Whether [a, a+n) is wholly mapped with MapPhysicalMemory (Horizon's alias-region memory is Normal, so it may
    /// be MapMemory's source as heap memory is: Pokémon Legends: Z-A maps its thread stacks from its allocator's
    /// pages in the alias region).
    fn in_phys(st: &State, a: u64, n: u64) -> bool {
        let mut runs: Vec<(u64, u64)> = st.phys.iter().map(|&(b, k)| (b, b + k)).collect();
        runs.sort_unstable();
        let mut at = a;
        for (lo, hi) in runs {
            if lo > at {
                break;
            }
            at = at.max(hi);
            if at >= a + n {
                return true;
            }
        }
        at >= a + n
    }

    /// svcMapMemory(dst, src, size): dst in the stack region, src heap, mapped alias-region memory or other mapped memory
    /// (a module's .bss: Pokémon Violet maps a thread stack from main's; Turbo checks no source state).
    pub fn map_memory(&self, dst: u64, src: u64, size: u64) -> u64 {
        if size == 0 || (dst | src | size) & (PAGE - 1) != 0 {
            return R_INVALID_SIZE;
        }
        let stack = self.space.stack;
        if dst < stack.base || dst + size > stack.end() {
            crate::log!("[RED] MapMemory {dst:#x} <- {src:#x} +{size:#x}: destination outside the stack region");
            return R_INVALID_ADDRESS;
        }
        let mut st = self.st.lock().unwrap();
        if !self.in_heap(&st, src, size) && !Self::in_phys(&st, src, size) && !rmem::readable(src, size) {
            crate::log!("[RED] MapMemory {dst:#x} <- {src:#x} +{size:#x}: source is not mapped");
            return R_INVALID_STATE;
        }
        let overl = |a: u64, b: u64, c: u64, d: u64| a < c + d && c < a + b;
        if st.maps.iter().any(|m| overl(m.dst, m.size, dst, size) || overl(m.src, m.size, src, size)) {
            return R_INVALID_STATE;
        }
        if !commit_rw(dst, size) {
            nxrt::rt::fatal(&format!("os: MapMemory commit of {dst:#x}+{size:#x} failed"));
        }
        prepare_locked(self, &st, src, size);
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, size as usize) };
        st.maps.push(Mapping { dst, src, size });
        0
    }

    pub fn unmap_memory(&self, dst: u64, src: u64, size: u64) -> u64 {
        let mut st = self.st.lock().unwrap();
        let Some(i) = st.maps.iter().position(|m| m.dst == dst && m.src == src && m.size == size) else {
            crate::log!("[RED] UnmapMemory {dst:#x} <- {src:#x} +{size:#x}: no such mapping");
            return R_INVALID_STATE;
        };
        prepare_locked(self, &st, src, size);
        unsafe { core::ptr::copy_nonoverlapping(dst as *const u8, src as *mut u8, size as usize) };
        rmem::decommit(dst, size);
        st.maps.remove(i);
        0
    }

    /// svcMapPhysicalMemory / svcUnmapPhysicalMemory in the alias region.
    pub fn physical(&self, a: u64, size: u64, map: bool) -> u64 {
        let al = self.space.alias;
        if size == 0 || (a | size) & (PAGE - 1) != 0 || a < al.base || a + size > al.end() {
            crate::log!("[RED] {}PhysicalMemory {a:#x}+{size:#x} outside the alias region", if map { "Map" } else { "Unmap" });
            return R_INVALID_ADDRESS;
        }
        let mut st = self.st.lock().unwrap();
        // the bytes that change state, from the runs mapped now
        let mapped_in: u64 = st
            .phys
            .iter()
            .map(|&(b, n)| {
                let lo = b.max(a);
                let hi = (b + n).min(a + size);
                hi.saturating_sub(lo)
            })
            .sum();
        if map {
            let need = size - mapped_in;
            if self.fig.used_beside_heap + st.heap_size + st.phys_bytes + need > self.fig.pool && map_limit() {
                return R_LIMIT_REACHED;
            }
            if !commit_rw(a, size) {
                nxrt::rt::fatal(&format!("os: MapPhysicalMemory commit of {a:#x}+{size:#x} failed"));
            }
            st.phys_bytes += need;
            st.phys.push((a, size));
        } else {
            rmem::decommit(a, size);
            st.phys_bytes -= mapped_in;
            let mut out = vec![];
            for &(b, n) in &st.phys {
                if b < a {
                    out.push((b, (b + n).min(a) - b));
                }
                if b + n > a + size {
                    let lo = b.max(a + size);
                    out.push((lo, b + n - lo));
                }
            }
            st.phys = out;
        }
        st.phys.sort();
        // merge adjacent and overlapping runs
        let mut merged: Vec<(u64, u64)> = vec![];
        for &(b, n) in &st.phys {
            if let Some(last) = merged.last_mut() {
                if b <= last.0 + last.1 {
                    let end = (last.0 + last.1).max(b + n);
                    last.1 = end - last.0;
                    continue;
                }
            }
            merged.push((b, n));
        }
        st.phys = merged;
        0
    }

    /// Backs a shared memory view at the address the guest chose.
    pub fn map_shared(&self, a: u64, size: u64) -> bool {
        let ok = commit_rw(a, size);
        if ok {
            self.st.lock().unwrap().shared.push((a, size));
        }
        ok
    }

    pub fn unmap_shared(&self, a: u64, size: u64) {
        let mut st = self.st.lock().unwrap();
        st.shared.retain(|&(b, n)| !(b == a && n == size));
        rmem::decommit(a, size);
    }

    /// svcQueryMemory.
    pub fn query(&self, a: u64) -> MemoryInfo {
        let st = self.st.lock().unwrap();
        let mut own: Vec<(Range, u32, u32, u32)> = vec![];
        if st.heap_size > 0 {
            own.push((Range { base: self.space.heap.base, size: st.heap_size }, ST_NORMAL, 0, 3));
        }
        for m in &st.maps {
            own.push((Range { base: m.dst, size: m.size }, ST_STACK, 0, 3));
        }
        for &(b, n) in &st.phys {
            own.push((Range { base: b, size: n }, ST_NORMAL, 0, 3));
        }
        for &(b, n) in &st.shared {
            own.push((Range { base: b, size: n }, ST_SHARED, 0, 3));
        }
        own.sort_by_key(|r| r.0.base);
        // a heap range lent to MapMemory: Normal, locked, no access
        for m in &st.maps {
            if a >= m.src && a < m.src + m.size {
                return MemoryInfo { addr: m.src, size: m.size, state: ST_NORMAL, attr: ATTR_LOCKED, perm: 0, ..Default::default() };
            }
        }
        if let Some(r) = own.iter().find(|r| r.0.contains(a)) {
            // the heap is split around lent ranges
            let (mut lo, mut hi) = (r.0.base, r.0.end());
            if r.1 == ST_NORMAL && r.0.base == self.space.heap.base {
                for m in &st.maps {
                    if m.src + m.size <= a && m.src + m.size > lo {
                        lo = m.src + m.size;
                    }
                    if m.src > a && m.src < hi {
                        hi = m.src;
                    }
                }
            }
            return MemoryInfo { addr: lo, size: hi - lo, state: r.1, attr: r.2, perm: r.3, ..Default::default() };
        }
        let q = self.space.query(a);
        if q.state != ST_FREE {
            return q;
        }
        let (mut lo, mut hi) = (q.addr, q.addr.saturating_add(q.size));
        for r in &own {
            if r.0.end() <= a && r.0.end() > lo {
                lo = r.0.end();
            }
            if r.0.base > a && r.0.base < hi {
                hi = r.0.base;
            }
        }
        // the address space ends at 2^39: beyond it nothing is mapped (Inaccessible, as Horizon reports)
        if a >= rmem::ADDRESS_LIMIT {
            return MemoryInfo { addr: rmem::ADDRESS_LIMIT, size: 0u64.wrapping_sub(rmem::ADDRESS_LIMIT), state: 0x10, ..Default::default() };
        }
        hi = hi.min(rmem::ADDRESS_LIMIT);
        MemoryInfo { addr: lo, size: hi - lo, state: ST_FREE, ..Default::default() }
    }

    /// svcGetInfo for the memory ids; None for ids this layer does not answer.
    pub fn info(&self, id: u32) -> Option<u64> {
        let st = self.st.lock().unwrap();
        let used = st.heap_size + self.fig.used_beside_heap + st.phys_bytes;
        Some(match id {
            2 => self.space.alias.base,
            3 => self.space.alias.size,
            4 => self.space.heap.base,
            5 => self.space.heap.size,
            6 | 21 => self.fig.pool,
            7 | 22 => used,
            12 => self.space.aslr.base,
            13 => rmem::ADDRESS_LIMIT - self.space.aslr.base,
            14 => self.space.stack.base,
            15 => self.space.stack.size,
            16 => self.fig.sys_resource,
            17 => {
                if self.fig.sys_resource != 0 {
                    0x40_0000
                } else {
                    0
                }
            }
            _ => return None,
        })
    }
}

/// Whether a map past the application pool fails with ResultLimitReached: Turbo's titles (b316a9f: Pokémon Legends:
/// Z-A, Ocarina of Time) and every profile with nxnative's `mem_map_limit on` (Pokémon Scarlet / Violet: start-up
/// probes all allocatable memory and gives the cached pages back only after a map fails); `SWR_MAP_LIMIT=1/0` for any.
fn map_limit() -> bool {
    if let Some(v) = std::env::var("SWR_MAP_LIMIT").ok().filter(|v| !v.is_empty()) {
        return v.trim().parse::<i64>().map_or(true, |n| n > 0);
    }
    let p = crate::profile::get();
    p.map_limit || matches!(p.program_id, 0x0100_F430_08C4_4000 | 0x0400_4860_24DE_C000)
}

fn prepare_locked(m: &Memory, st: &State, a: u64, n: u64) {
    let hb = m.space.heap.base;
    let lo = a.max(hb) & !(PAGE - 1);
    let hi = rmem::align_up((a + n).min(hb + st.heap_size), PAGE);
    if lo < hi {
        commit_rw(lo, hi - lo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One space for the whole test binary (reservations are process-wide).
    pub(crate) fn test_memory() -> &'static Memory {
        static M: OnceLock<&'static Memory> = OnceLock::new();
        M.get_or_init(|| {
            let mods = Range { base: 0x6C_0000_0000, size: 0x10_0000 };
            let space: &'static Space = Box::leak(Box::new(Space::new(mods, 0x6D_0000_0000, false).expect("space")));
            init(space, Figures { pool: 0xCD50_0000, used_beside_heap: 64 << 20, sys_resource: 0 })
        })
    }

    #[test]
    fn heap_is_lazy_and_maps_copy() {
        let m = test_memory();
        let h = m.set_heap_size(0x400_0000).unwrap();
        assert_eq!(h, m.space.heap.base);
        assert!(!rmem::readable(h + 0x10_0000, 8), "not committed before the first touch");
        unsafe { *((h + 0x10_0008) as *mut u64) = 0x55 }; // faults, the handler commits, the store retries
        assert_eq!(unsafe { *((h + 0x10_0008) as *const u64) }, 0x55);
        assert_eq!(m.query(h + 0x20).state, ST_NORMAL);
        assert_eq!(m.set_heap_size(0x123), Err(R_INVALID_SIZE));

        let dst = m.space.stack.base + 0x80_0000;
        unsafe { *((h + 0x10_0010) as *mut u64) = 7 };
        assert_eq!(m.map_memory(dst, h + 0x10_0000, 0x2000), 0);
        assert_eq!(unsafe { *((dst + 0x10) as *const u64) }, 7, "contents moved");
        let q = m.query(dst + 0x1000);
        assert_eq!((q.addr, q.size, q.state), (dst, 0x2000, ST_STACK));
        let s = m.query(h + 0x10_0800);
        assert_eq!((s.addr, s.size, s.attr), (h + 0x10_0000, 0x2000, ATTR_LOCKED));
        let before = m.query(h);
        assert_eq!(before.addr + before.size, h + 0x10_0000, "the heap is split around the lent range");
        unsafe { *((dst + 0x18) as *mut u64) = 9 };
        assert_eq!(m.map_memory(dst, h + 0x20_0000, 0x1000), R_INVALID_STATE, "overlap");
        assert_eq!(m.unmap_memory(dst, h + 0x10_0000, 0x2000), 0);
        assert_eq!(unsafe { *((h + 0x10_0018) as *const u64) }, 9, "contents moved back");
        assert_eq!(m.query(dst).state, ST_FREE);
    }

    #[test]
    fn physical_memory_and_info() {
        let m = test_memory();
        let a = m.space.alias.base + 0x100_0000;
        assert_eq!(m.physical(a, 0x4000, true), 0);
        assert_eq!(m.physical(a + 0x2000, 0x4000, true), 0);
        assert_eq!(m.query(a + 0x5000).state, ST_NORMAL);
        let dst = m.space.stack.base + 0x40_0000;
        assert_eq!(m.map_memory(dst, a + 0x1000, 0x4000), 0, "mapped alias memory is a MapMemory source");
        assert_eq!(m.unmap_memory(dst, a + 0x1000, 0x4000), 0);
        assert_eq!(m.map_memory(dst, a + 0x8000, 0x1000), R_INVALID_STATE, "unmapped alias memory is not");
        let used0 = m.info(7).unwrap();
        assert_eq!(m.physical(a, 0x6000, false), 0);
        assert_eq!(m.info(7).unwrap(), used0 - 0x6000);
        assert_eq!(m.query(a).state, ST_FREE);
        assert_eq!(m.info(6), Some(0xCD50_0000));
        assert_eq!(m.physical(0x1000, 0x1000, true), R_INVALID_ADDRESS);
    }
}

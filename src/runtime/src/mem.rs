//! Guest memory on Windows: guest addresses are host addresses, all below 2^39.
//! VirtualAlloc reservations at fixed addresses, `MEM_WRITE_WATCH` with GetWriteWatch; a stack+heap block searched
//! from 4 GiB up, an alias region above it, the module window, a kernel region and the host-function window; 4 KiB
//! commit; decommitted pages come back zeroed.
//!
//! Fixed part of the map:
//!   modules           from 0x70_0000_0000 (the translator's layout.json / layout.h)
//!   kernel region     `KERNEL_BASE`, 256 MiB: TLS regions (TPIDRRO) and host objects the guest reads
//!   host functions    `hostfn::HOSTFN_BASE`, 1 MiB, never accessible
//! Placed at start-up: the stack+heap block (write-watched) and the alias region (write-watched).
//!
//! Physical memory in the alias region (MapPhysicalMemory) is counted against the application pool with the heap;
//! with the map limit on, a map past the pool fails with ResultLimitReached as on Horizon. The heap can be committed
//! lazily, a page group at its first touch (`set_lazy_heap`). What is left out: frozen regions, read traps for
//! GPU-written pages, stack moves (MapMemory).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::System::Memory::*;
// Not in windows-sys' Memory module: SystemServices values.
const MEM_WRITE_WATCH: u32 = 0x20_0000;
const WRITE_WATCH_FLAG_RESET: u32 = 1;

pub const PAGE: u64 = 0x1000;
pub const GIB: u64 = 1 << 30;
pub const ADDRESS_LIMIT: u64 = 1 << 39;
pub const KERNEL_BASE: u64 = 0x7F_0000_0000;
pub const KERNEL_SIZE: u64 = 0x1000_0000;
const LOW_SEARCH_FROM: u64 = 0x1_0000_0000;
const LOW_SIZE: u64 = 12 * GIB;
const STACK_OFF: u64 = 0x1000_0000;
const STACK_SIZE: u64 = 0x1000_0000;
const HEAP_OFF: u64 = STACK_OFF + STACK_SIZE + 0x1000_0000;
/// The heap region: room for a Switch 2 pool (9 GiB, `process::app_pool`) less what the SDK keeps beside the heap.
pub const HEAP_CAP: u64 = 10 * GIB;
/// The alias region: Horizon's 64 GiB for a 39-bit space (Turbo's `g_alias_size`). Only address space: Pokémon Legends:
/// Z-A's nninitStartup puts a StandardAllocator over 12 GiB of it (nn::diag abort in the SDK with 4 GiB).
const ALIAS_SIZE: u64 = 64 * GIB;
const ALIAS_ALIGN: u64 = 4 * GIB;
/// Application memory pool a Switch 1 title sees (GetInfo TotalMemorySize): 3285 MiB.
pub const DEFAULT_POOL: u64 = 3285 << 20;

// Horizon memory states and permissions (QueryMemory).
pub const ST_FREE: u32 = 0;
pub const ST_CODE: u32 = 3;
pub const ST_CODEDATA: u32 = 4;
pub const ST_NORMAL: u32 = 5;
pub const ST_STACK: u32 = 0xB;
pub const ST_THREADLOCAL: u32 = 0xC;
pub const PERM_NONE: u32 = 0;
pub const PERM_R: u32 = 1;
pub const PERM_RW: u32 = 3;
pub const PERM_RX: u32 = 5;

// ---- the platform layer (win32_mem.c)

/// Reserves [addr, addr+n) exactly there (no access), optionally write-watched.
pub fn reserve(addr: u64, n: u64, watch: bool) -> bool {
    let flags = MEM_RESERVE | if watch { MEM_WRITE_WATCH } else { 0 };
    let p = unsafe { VirtualAlloc(addr as _, n as usize, flags, PAGE_NOACCESS) };
    if !p.is_null() && p as u64 != addr {
        unsafe { VirtualFree(p, 0, MEM_RELEASE) };
        return false;
    }
    !p.is_null()
}

/// Reserves n bytes wherever the OS likes (for tests and host tables), optionally write-watched.
pub fn reserve_anywhere(n: u64, watch: bool) -> Option<u64> {
    let flags = MEM_RESERVE | if watch { MEM_WRITE_WATCH } else { 0 };
    let p = unsafe { VirtualAlloc(core::ptr::null(), n as usize, flags, PAGE_NOACCESS) };
    (!p.is_null()).then_some(p as u64)
}

pub fn release(addr: u64) {
    unsafe { VirtualFree(addr as _, 0, MEM_RELEASE) };
}

pub fn commit(addr: u64, n: u64) -> bool {
    n == 0 || !unsafe { VirtualAlloc(addr as _, n as usize, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

pub fn decommit(addr: u64, n: u64) -> bool {
    n == 0 || unsafe { VirtualFree(addr as _, n as usize, MEM_DECOMMIT) } != 0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prot {
    None,
    Read,
    ReadWrite,
}

pub fn protect(addr: u64, n: u64, p: Prot) -> bool {
    let p = match p {
        Prot::None => PAGE_NOACCESS,
        Prot::Read => PAGE_READONLY,
        Prot::ReadWrite => PAGE_READWRITE,
    };
    let mut old = 0;
    n == 0 || unsafe { VirtualProtect(addr as _, n as usize, p, &mut old) } != 0
}

/// Zeroed host memory (committed on first touch by the OS), for the runtime's own tables.
pub fn host_alloc(n: u64) -> Option<*mut u8> {
    let p = unsafe { VirtualAlloc(core::ptr::null(), n as usize, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
    (!p.is_null()).then_some(p as *mut u8)
}

pub fn host_protect_readonly(addr: u64, n: u64) {
    if !protect(addr, n, Prot::Read) {
        crate::rt::fatal("memory: sealing a host table failed");
    }
}

/// Every byte of [addr, addr+n) is committed and readable.
pub fn readable(addr: u64, n: u64) -> bool {
    let mut a = addr;
    let end = addr.saturating_add(n.max(1));
    while a < end {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(a as _, &mut mbi, core::mem::size_of_val(&mbi)) } == 0 || mbi.State != MEM_COMMIT {
            return false;
        }
        if mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0 {
            return false;
        }
        a = mbi.BaseAddress as u64 + mbi.RegionSize as u64;
    }
    true
}

/// The pages of [addr, addr+n) written since the last reset (GetWriteWatch; the range must lie in one write-watched
/// reservation). `reset` clears the record.
pub fn written_pages(addr: u64, n: u64, reset: bool) -> Option<Vec<u64>> {
    let max = n.div_ceil(PAGE) as usize;
    let mut pages = vec![0u64; max.max(1)];
    let mut got = pages.len();
    let mut gran = 0u32;
    let r = unsafe {
        GetWriteWatch(
            if reset { WRITE_WATCH_FLAG_RESET } else { 0 },
            addr as _,
            n as usize,
            pages.as_mut_ptr() as *mut *mut core::ffi::c_void,
            &mut got,
            &mut gran,
        )
    };
    if r != 0 {
        return None;
    }
    pages.truncate(got);
    Some(pages)
}

// ---- the guest address space (memory.c)

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub base: u64,
    pub size: u64,
}

impl Range {
    pub fn end(&self) -> u64 {
        self.base + self.size
    }
    pub fn contains(&self, a: u64) -> bool {
        a.wrapping_sub(self.base) < self.size
    }
}

/// Horizon's MemoryInfo (QueryMemory output), 0x28 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryInfo {
    pub addr: u64,
    pub size: u64,
    pub state: u32,
    pub attr: u32,
    pub perm: u32,
    pub ipc_refcount: u32,
    pub device_refcount: u32,
    pub padding: u32,
}

#[derive(Clone, Copy, Debug)]
struct Region {
    r: Range,
    state: u32,
    perm: u32,
}

struct State {
    heap_size: u64,
    kernel_top: u64,
    stack_top: u64,
    regions: Vec<Region>,
    free_tls: Vec<u64>,
}

pub struct Space {
    pub modules: Range,
    pub aslr: Range,
    pub stack: Range,
    pub heap: Range,
    pub alias: Range,
    pub kernel: Range,
    /// The application pool (GetInfo TotalMemorySize): what the heap and physical memory may take together.
    pub pool: u64,
    pool_override: AtomicU64,
    /// Bytes of the alias region mapped with MapPhysicalMemory.
    phys_bytes: AtomicU64,
    /// A map past the pool fails with ResultLimitReached (else it is granted beyond the pool).
    map_limit: AtomicBool,
    /// The heap is committed at first touch (`lazy_fault`) instead of at SetHeapSize.
    lazy: AtomicBool,
    /// The heap's current size, for the fault handler (no lock there).
    heap_end: AtomicU64,
    st: Mutex<State>,
}

/// Lazy commit grain: a fault commits the 64 KiB around it.
const LAZY_GRAIN: u64 = 0x1_0000;

pub const R_OK: u32 = 0;
pub const R_LIMIT_REACHED: u32 = 0x10801;
pub const R_INVALID_SIZE: u32 = 0xca01;
pub const R_INVALID_STATE: u32 = 0xd401;
pub const R_INVALID_ADDRESS: u32 = 0xcc01;

impl Space {
    /// Claims the address map: the module window [mods.base, mods.end), the kernel region, the host-function window,
    /// then a stack+heap block searched from 4 GiB up below the modules, and the alias region above it.
    /// `kernel_base` is `KERNEL_BASE` in a game process (tests pass their own).
    pub fn new(mods: Range, kernel_base: u64, reserve_hostfn: bool) -> Result<Space, String> {
        let mods = Range { base: mods.base & !0xFFFF, size: align_up(mods.end(), 0x10000) - (mods.base & !0xFFFF) };
        if mods.end() > kernel_base || kernel_base + KERNEL_SIZE > ADDRESS_LIMIT {
            return Err(format!("memory: modules end at {:#x}, inside the kernel region", mods.end()));
        }
        if !reserve(mods.base, mods.size, false) {
            return Err(format!("memory: the module window {:#x}+{:#x} is taken", mods.base, mods.size));
        }
        if !reserve(kernel_base, KERNEL_SIZE, false) {
            return Err("memory: the kernel region is taken".into());
        }
        if reserve_hostfn && !reserve(crate::hostfn::HOSTFN_BASE, crate::hostfn::HOSTFN_SIZE, false) {
            return Err("memory: the host-function window is taken".into());
        }
        let mut low = 0;
        let mut at = LOW_SEARCH_FROM;
        while at + LOW_SIZE <= mods.base && low == 0 {
            if reserve(at, LOW_SIZE, true) {
                low = at;
            }
            at += GIB;
        }
        if low == 0 {
            return Err("memory: no free 8 GiB stack and heap block below the modules".into());
        }
        let mut alias = 0;
        let mut at = align_up(low + LOW_SIZE, ALIAS_ALIGN);
        while at + ALIAS_SIZE <= mods.base && alias == 0 {
            if reserve(at, ALIAS_SIZE, true) {
                alias = at;
            }
            at += ALIAS_ALIGN;
        }
        if alias == 0 {
            return Err("memory: no free alias region below the modules".into());
        }
        Ok(Space {
            modules: mods,
            aslr: Range { base: low, size: ADDRESS_LIMIT - low },
            stack: Range { base: low + STACK_OFF, size: STACK_SIZE },
            heap: Range { base: low + HEAP_OFF, size: HEAP_CAP },
            alias: Range { base: alias, size: ALIAS_SIZE },
            kernel: Range { base: kernel_base, size: KERNEL_SIZE },
            pool: DEFAULT_POOL,
            pool_override: AtomicU64::new(0),
            phys_bytes: AtomicU64::new(0),
            map_limit: AtomicBool::new(false),
            lazy: AtomicBool::new(false),
            heap_end: AtomicU64::new(0),
            st: Mutex::new(State {
                heap_size: 0,
                kernel_top: kernel_base,
                stack_top: low + STACK_OFF,
                regions: vec![],
                free_tls: vec![],
            }),
        })
    }

    /// Commits a module's image read-write (the loader copies and relocates it, then `protect_module`).
    pub fn map_module(&self, base: u64, size: u64) -> Result<(), String> {
        if base < self.modules.base || base + size > self.modules.end() {
            return Err(format!("memory: module {base:#x}+{size:#x} outside the module window"));
        }
        if !commit(base, size) {
            return Err(format!("memory: commit of module {base:#x}+{size:#x} failed"));
        }
        Ok(())
    }

    /// Final permissions of a module segment, recorded for QueryMemory.
    pub fn protect_segment(&self, base: u64, size: u64, state: u32, perm: u32) -> Result<(), String> {
        if size == 0 {
            return Ok(());
        }
        let p = if perm & 2 != 0 { Prot::ReadWrite } else { Prot::Read };
        if !protect(base, size, p) {
            return Err(format!("memory: protect {base:#x}+{size:#x} failed"));
        }
        let mut st = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        st.regions.push(Region { r: Range { base, size }, state, perm });
        st.regions.sort_by_key(|r| r.r.base);
        Ok(())
    }

    /// The application pool now (the title's, once `set_pool` gave it).
    pub fn pool(&self) -> u64 {
        match self.pool_override.load(Ordering::Relaxed) {
            0 => self.pool,
            p => p,
        }
    }

    /// The title's application pool (its profile's `app_pool`).
    pub fn set_pool(&self, bytes: u64) {
        self.pool_override.store(bytes, Ordering::Relaxed);
    }

    /// Whether a map past the pool fails with ResultLimitReached.
    pub fn set_map_limit(&self, on: bool) {
        self.map_limit.store(on, Ordering::Relaxed);
    }

    /// Commit the heap at first touch instead of at SetHeapSize. Host code that hands guest heap memory to the OS
    /// (ReadFile into a guest buffer) must call `ensure_committed` first: the OS reports an uncommitted page as an
    /// error instead of faulting.
    pub fn set_lazy_heap(&self, on: bool) {
        self.lazy.store(on, Ordering::Relaxed);
    }

    /// Commits the heap pages of [a, a+n) that are not yet (lazy heap); other ranges are left alone.
    pub fn ensure_committed(&self, a: u64, n: u64) {
        if !self.lazy.load(Ordering::Relaxed) || n == 0 {
            return;
        }
        let (lo, hi) = (a.max(self.heap.base), (a + n).min(self.heap.base + self.heap_end.load(Ordering::Acquire)));
        if lo < hi {
            let c0 = lo & !(LAZY_GRAIN - 1);
            commit(c0, align_up(hi, LAZY_GRAIN).min(self.heap.base + self.heap_end.load(Ordering::Acquire)) - c0);
        }
    }

    /// The fault handler's question: `a` is an untouched page of a lazily committed heap; commits the 64 KiB
    /// around it. True: the access can be retried.
    pub fn lazy_fault(&self, a: u64) -> bool {
        if !self.lazy.load(Ordering::Relaxed) {
            return false;
        }
        let end = self.heap.base + self.heap_end.load(Ordering::Acquire);
        if a < self.heap.base || a >= end {
            return false;
        }
        let c0 = a & !(LAZY_GRAIN - 1);
        commit(c0, (c0 + LAZY_GRAIN).min(end) - c0)
    }

    /// svcSetHeapSize: the heap grows and shrinks at its base in 2 MiB steps; returns the heap's address. The heap
    /// and the mapped physical memory share the pool.
    pub fn set_heap_size(&self, size: u64) -> Result<u64, u32> {
        let mut st = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if size > HEAP_CAP || !size.is_multiple_of(0x20_0000) {
            return Err(R_INVALID_SIZE);
        }
        if size > st.heap_size && size + self.phys_bytes.load(Ordering::Relaxed) > self.pool() {
            return Err(R_LIMIT_REACHED);
        }
        let b = self.heap.base;
        let lazy = self.lazy.load(Ordering::Relaxed);
        if size > st.heap_size && !lazy && !commit(b + st.heap_size, size - st.heap_size) {
            crate::rt::fatal(&format!("memory: commit of the heap to {size:#x} failed"));
        }
        if size < st.heap_size && !decommit(b + size, st.heap_size - size) {
            crate::rt::fatal("memory: decommit of the heap failed");
        }
        st.heap_size = size;
        self.heap_end.store(size, Ordering::Release);
        Ok(b)
    }

    /// Committed bytes of the heap and of the alias region (physical memory maps), for the memory report.
    pub fn committed(&self) -> (u64, u64) {
        (Self::committed_in(self.heap.base, self.heap.size), Self::committed_in(self.alias.base, self.alias.size))
    }

    /// Committed bytes of [a, a+n).
    fn committed_in(a: u64, n: u64) -> u64 {
        let mut total = 0;
        let mut p = a;
        while p < a + n {
            let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
            // SAFETY: a query of our own address space
            if unsafe { VirtualQuery(p as *const _, &mut mbi, core::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
                break;
            }
            let end = (mbi.BaseAddress as u64 + mbi.RegionSize as u64).min(a + n);
            if mbi.State == MEM_COMMIT {
                total += end - p;
            }
            p = end;
        }
        total
    }

    /// svcMapPhysicalMemory / svcUnmapPhysicalMemory: pages of the alias region committed and released, counted
    /// against the pool with the heap. With the map limit on, a map that would take the pool past its size fails
    /// with ResultLimitReached: the SDK's allocator keeps freed pages mapped and maps more until a map fails; only
    /// then does it give its cached block back and try again (Pokemon Legends: Z-A, Ocarina of Time).
    pub fn map_physical(&self, a: u64, size: u64, map: bool) -> u32 {
        let al = self.alias;
        if size == 0 || (a | size) & (PAGE - 1) != 0 || a < al.base || a + size > al.end() {
            return R_INVALID_ADDRESS;
        }
        let _st = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mapped = Self::committed_in(a, size);
        if map {
            let need = size - mapped;
            let used = self.heap_end.load(Ordering::Relaxed) + self.phys_bytes.load(Ordering::Relaxed);
            if need > 0 && used + need > self.pool() && self.map_limit.load(Ordering::Relaxed) {
                static SAID: AtomicU64 = AtomicU64::new(0);
                if SAID.fetch_add(1, Ordering::Relaxed) < 8 {
                    crate::rt::log(&format!(
                        "memory: MapPhysicalMemory +{} MB refused: {} MB mapped, the application pool ({} MB) is full (ResultLimitReached: the SDK gives its cached pages back and tries again)",
                        need >> 20,
                        self.phys_bytes.load(Ordering::Relaxed) >> 20,
                        self.pool() >> 20
                    ));
                }
                return R_LIMIT_REACHED;
            }
            if !commit(a, size) {
                crate::rt::fatal(&format!("memory: MapPhysicalMemory commit of {a:#x}+{size:#x} failed"));
            }
            self.phys_bytes.fetch_add(need, Ordering::Relaxed);
        } else {
            decommit(a, size);
            self.phys_bytes.fetch_sub(mapped.min(self.phys_bytes.load(Ordering::Relaxed)), Ordering::Relaxed);
        }
        R_OK
    }

    /// Bytes mapped with MapPhysicalMemory.
    pub fn physical_bytes(&self) -> u64 {
        self.phys_bytes.load(Ordering::Relaxed)
    }

    pub fn heap_size(&self) -> u64 {
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).heap_size
    }

    /// A guest stack of `size` bytes in the stack region (the main thread's), committed. Returns its base.
    pub fn alloc_stack(&self, size: u64) -> Result<u64, String> {
        let size = align_up(size, PAGE);
        let mut st = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let a = st.stack_top;
        if a + size > self.stack.end() {
            return Err("memory: the stack region is full".into());
        }
        if !commit(a, size) {
            return Err("memory: commit of a stack failed".into());
        }
        st.stack_top = a + size + PAGE; // an uncommitted guard page between stacks
        st.regions.push(Region { r: Range { base: a, size }, state: ST_STACK, perm: PERM_RW });
        st.regions.sort_by_key(|r| r.r.base);
        Ok(a)
    }

    /// Zeroed, committed memory in the kernel region (host objects the guest reads).
    pub fn kernel_alloc(&self, size: u64, align: u64) -> u64 {
        let mut st = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let a = align_up(st.kernel_top, align.max(16));
        let end = a + size;
        if end > self.kernel.end() {
            crate::rt::fatal("memory: the kernel region is full");
        }
        let (c0, c1) = (st.kernel_top & !(PAGE - 1), align_up(end, PAGE));
        if !commit(c0, c1 - c0) {
            crate::rt::fatal("memory: kernel region commit failed");
        }
        st.kernel_top = end;
        a
    }

    /// A thread's 0x200-byte TLS region (TPIDRRO_EL0), zeroed; reused after `free_tls`.
    pub fn alloc_tls(&self) -> u64 {
        if let Some(a) = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).free_tls.pop() {
            // SAFETY: a region this space handed out, committed.
            unsafe { core::ptr::write_bytes(a as *mut u8, 0, 0x200) };
            return a;
        }
        self.kernel_alloc(0x200, 0x200)
    }

    pub fn free_tls(&self, a: u64) {
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).free_tls.push(a);
    }

    /// svcQueryMemory: the region holding `a`, from the runtime's own table (heap, stacks, module segments), free
    /// space between them clipped to the neighbours.
    pub fn query(&self, a: u64) -> MemoryInfo {
        let st = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut regions = st.regions.clone();
        if st.heap_size > 0 {
            regions.push(Region { r: Range { base: self.heap.base, size: st.heap_size }, state: ST_NORMAL, perm: PERM_RW });
        }
        regions.sort_by_key(|r| r.r.base);
        if let Some(r) = regions.iter().find(|r| r.r.contains(a)) {
            return MemoryInfo { addr: r.r.base, size: r.r.size, state: r.state, perm: r.perm, ..Default::default() };
        }
        let lo = regions.iter().filter(|r| r.r.end() <= a).map(|r| r.r.end()).max().unwrap_or(0);
        let hi = regions.iter().filter(|r| r.r.base > a).map(|r| r.r.base).min().unwrap_or(u64::MAX);
        MemoryInfo { addr: lo, size: hi - lo, state: ST_FREE, perm: PERM_NONE, ..Default::default() }
    }

    /// svcGetInfo for the process memory ids.
    pub fn info(&self, id: u32) -> Option<u64> {
        let used = self.heap_size() + self.modules.size;
        Some(match id {
            2 => self.alias.base,
            3 => self.alias.size,
            4 => self.heap.base,
            5 => self.heap.size,
            6 | 21 => self.pool(),
            7 | 22 => used + self.physical_bytes(),
            12 => self.aslr.base,
            13 => self.aslr.size,
            14 => self.stack.base,
            15 => self.stack.size,
            16 | 17 => 0,
            _ => return None,
        })
    }

    /// Pages the CPU wrote in [addr, addr+n) since the last call (the range must lie in the stack+heap block or the
    /// alias region).
    pub fn written_pages(&self, addr: u64, n: u64) -> Option<Vec<u64>> {
        let low = Range { base: self.aslr.base, size: LOW_SIZE };
        let inside = |r: Range| addr >= r.base && addr + n <= r.end();
        if !inside(low) && !inside(self.alias) {
            return None;
        }
        written_pages(addr, n, true)
    }
}

pub fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_info_is_horizons() {
        assert_eq!(core::mem::size_of::<MemoryInfo>(), 0x28);
        assert_eq!(core::mem::offset_of!(MemoryInfo, state), 0x10);
        assert_eq!(core::mem::offset_of!(MemoryInfo, perm), 0x18);
    }

    #[test]
    fn write_watch_reports_exactly_the_written_pages() {
        let n = 64 * PAGE;
        let a = reserve_anywhere(n, true).unwrap();
        assert!(commit(a, n));
        assert_eq!(written_pages(a, n, true).unwrap(), Vec::<u64>::new());
        for p in [3u64, 7, 8, 40] {
            unsafe { *((a + p * PAGE + 123) as *mut u8) = 1 };
        }
        let _ = unsafe { *((a + 9 * PAGE) as *const u8) }; // a read is no write
        let got = written_pages(a, n, true).unwrap();
        assert_eq!(got, [3u64, 7, 8, 40].map(|p| a + p * PAGE));
        assert_eq!(written_pages(a, n, true).unwrap(), Vec::<u64>::new(), "reset");
        assert!(decommit(a, n));
        assert!(commit(a, PAGE));
        assert_eq!(unsafe { *((a + 123) as *const u8) }, 0, "decommitted pages come back zeroed");
        release(a);
    }

    #[test]
    fn space_layout_heap_query_info() {
        // A module window and kernel region of the test's own, high in the 39-bit space.
        let mods = Range { base: 0x7C_0000_0000, size: 0x30_0000 };
        let s = Space::new(mods, 0x7D_0000_0000, false).expect("reservation below 2^39");
        assert!(s.stack.end() <= s.heap.base && s.heap.end() <= s.aslr.base + LOW_SIZE);
        assert!(s.alias.base >= s.aslr.base + LOW_SIZE && s.alias.end() <= mods.base);
        assert!(s.alias.end() <= ADDRESS_LIMIT);

        s.map_module(mods.base, 0x3000).unwrap();
        unsafe { *((mods.base + 0x2000) as *mut u64) = 5 };
        s.protect_segment(mods.base, 0x1000, ST_CODE, PERM_RX).unwrap();
        s.protect_segment(mods.base + 0x1000, 0x2000, ST_CODEDATA, PERM_RW).unwrap();
        assert!(readable(mods.base, 0x3000));
        let q = s.query(mods.base + 0x10);
        assert_eq!((q.addr, q.size, q.state, q.perm), (mods.base, 0x1000, ST_CODE, PERM_RX));
        let q = s.query(mods.base + 0x5000);
        assert_eq!((q.addr, q.state), (mods.base + 0x3000, ST_FREE));

        assert_eq!(s.set_heap_size(0x1234), Err(R_INVALID_SIZE));
        let h = s.set_heap_size(0x40_0000).unwrap();
        assert_eq!(h, s.heap.base);
        unsafe { *((h + 0x3F_FFF8) as *mut u64) = 7 };
        let q = s.query(h + 0x100);
        assert_eq!((q.addr, q.size, q.state), (h, 0x40_0000, ST_NORMAL));
        let w = s.written_pages(h, 0x40_0000).unwrap();
        assert_eq!(w, vec![h + 0x3F_F000]);
        s.set_heap_size(0x20_0000).unwrap();
        assert!(!readable(h + 0x20_0000, 8), "shrunk heap is decommitted");
        assert_eq!(s.info(4), Some(s.heap.base));
        assert_eq!(s.info(14), Some(s.stack.base));

        let st = s.alloc_stack(0x10000).unwrap();
        assert!(s.stack.contains(st));
        assert_eq!(s.query(st + 0x8000).state, ST_STACK);
        let t1 = s.alloc_tls();
        let t2 = s.alloc_tls();
        assert_eq!(t2 - t1, 0x200);
        unsafe { *(t1 as *mut u64) = 9 };
        s.free_tls(t1);
        assert_eq!(s.alloc_tls(), t1);
        assert_eq!(unsafe { *(t1 as *const u64) }, 0, "a reused TLS region is zeroed");
    }
}

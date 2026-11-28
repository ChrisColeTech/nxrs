//! the 32-bit guest memory: a region table plus the two page tables recomp.h designs (2^20 entries of
//! 4 KiB pages each) over one host reservation covering the whole guest space, pages committed lazily
//! a 64 KiB group at a time on first touch (the shape of `runtime/src/mem.rs`'s lazy heap, not its
//! code). GPU-visible pages (VRAM) keep their `write_pages` entry null so every write arrives at the
//! host's callbacks — the explicit-write-watch the ABI designs for (docs/interface.md §Memory).

use std::sync::Mutex;

/// page tables of 2^20 pointers, one per 4 KiB page of the 32-bit space
pub const PAGES: usize = 1 << 20;
const PAGE: u32 = 0x1000;
/// pages commit in groups of 16 (64 KiB), one host commit each
const GROUP_PAGES: u32 = 16;

/// how mapped memory may be read and written (bit0 R, bit1 W: 3dbrew MemoryPermission, write without
/// read is invalid on the real kernel)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    None,
    R,
    Rw,
}

impl Perm {
    fn readable(self) -> bool {
        self != Perm::None
    }
    fn writable(self) -> bool {
        self == Perm::Rw
    }
}

/// 3dbrew's MemoryState values (Memory_Management): what QueryMemory reports
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Free,
    Reserved,
    Io,
    Static,
    Code,
    Private,
    Shared,
    Continuous,
    Aliased,
    Alias,
    AliasCode,
    Locked,
}

/// why the memory is there, for the logs and the watched-write decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// ExeFS .code segments: text and rodata read-only, data writable
    Exe,
    Heap,
    Linear,
    Stack,
    /// shared memory mapped in the 0x10000000 window
    Shared,
    Vram,
    Mmio,
    Dsp,
    /// the kernel configuration page at 0x1FF80000
    Config,
    /// the shared system configuration page at 0x1FF81000
    SharedConfig,
    Tls,
}

/// what a host write into watched memory does besides landing in the backing store; the GPU wave
/// (W4) takes the hook, W2 counts
pub type WatchHook = Box<dyn Fn(u32, u64) + Send + Sync>;

#[derive(Debug, Clone, Copy)]
struct Region {
    base: u32,
    size: u32,
    perm: Perm,
    state: State,
    kind: Kind,
}

impl Region {
    fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size
    }
    fn overlaps(&self, base: u32, size: u32) -> bool {
        base < self.base.wrapping_add(self.size) && self.base < base.wrapping_add(size)
    }
    fn covers_page(&self, page: u32) -> bool {
        page * PAGE >= self.base && page * PAGE < self.base.wrapping_add(self.size)
    }
}

/// what the memory answered outside the tables; the boot report's memory lines
#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub faults: u64,
    /// the last faulting address (the boot log's first suspect)
    pub last_fault: u32,
    pub mmio_reads: u64,
    pub mmio_writes: u64,
    pub watched_writes: u64,
    pub committed_groups: u64,
}

struct Inner {
    regions: Vec<Region>,
    committed: std::collections::HashSet<u32>,
    hook: Option<WatchHook>,
    stats: Stats,
}

pub struct Memory {
    /// page i of the guest lives at base + i * 0x1000, null while the page has no committed backing;
    /// behind a cell because entry fills happen from the callbacks and under the inner lock
    tables: core::cell::UnsafeCell<Tables>,
    /// the reservation (guest addr + base = host addr), never dereferenced where nothing is committed
    base: *mut u8,
    inner: Mutex<Inner>,
}

struct Tables {
    read: Box<[*mut u8]>,
    write: Box<[*mut u8]>,
}

// safe: every pointer is only dereferenced through committed backing; the cell's writes are the
// entry fills the ABI allows (idempotent pointers, or empties at the svc boundary from unmap)
unsafe impl Send for Memory {}
unsafe impl Sync for Memory {}

/// QueryMemory's answer (3dbrew Memory_Management: base, size, perm, state)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryInfo {
    pub base: u32,
    pub size: u32,
    pub perm: Perm,
    pub state: State,
}

impl Memory {
    /// reserves the whole guest space, page tables of nulls behind it
    pub fn new() -> Result<Memory, String> {
        // safe: the reservation has a guard page set and nothing maps into it but commits below
        let base = windows_alloc(1 << 32).ok_or("could not reserve the 4 GiB guest space")?;
        Ok(Memory {
            tables: core::cell::UnsafeCell::new(Tables {
                read: vec![std::ptr::null_mut(); PAGES].into_boxed_slice(),
                write: vec![std::ptr::null_mut(); PAGES].into_boxed_slice(),
            }),
            base,
            inner: Mutex::new(Inner {
                regions: Vec::new(),
                committed: std::collections::HashSet::new(),
                hook: None,
                stats: Stats::default(),
            }),
        })
    }

    /// the read table's entry for a page number (what the generated code walks)
    pub fn read_entry(&self, page: u32) -> *mut u8 {
        // safe: entry reads race only with same-value fills
        unsafe { (*self.tables.get()).read[page as usize] }
    }

    /// the write table's entry for a page number
    pub fn write_entry(&self, page: u32) -> *mut u8 {
        // safe: same as read_entry
        unsafe { (*self.tables.get()).write[page as usize] }
    }

    /// the tables themselves, for a Context's pointers (the fills stay this host's business)
    pub fn read_table(&self) -> *const *mut u8 {
        unsafe { (*self.tables.get()).read.as_ptr() }
    }

    pub fn write_table(&self) -> *const *mut u8 {
        unsafe { (*self.tables.get()).write.as_ptr() }
    }

    /// maps a region; the pages stay uncommitted until first touched (lazy commit)
    /// overlaps are refused: a map over a live mapping is a kernel error, not a shadow
    pub fn map(&self, base: u32, size: u32, perm: Perm, state: State, kind: Kind) -> Result<(), String> {
        if size == 0 || base.checked_add(size).is_none() {
            return Err(format!("a {kind:?} region of {size} bytes at 0x{base:08X} is not a range"));
        }
        if base & (PAGE - 1) != 0 || size & (PAGE - 1) != 0 {
            return Err(format!("a {kind:?} region at 0x{base:08X} is not page aligned"));
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.regions.iter().any(|r| r.overlaps(base, size)) {
            return Err(format!("a {kind:?} region at 0x{base:08X} overlaps a live mapping"));
        }
        inner.regions.push(Region { base, size, perm, state, kind });
        Ok(())
    }

    /// svcControlMemory COMMIT: a range already held by one mapping changes state/perm in place
    /// (the title committing pages of its reserved heap), otherwise it becomes a fresh mapping
    pub fn commit(&self, base: u32, size: u32, perm: Perm, state: State, kind: Kind) -> Result<(), String> {
        if size != 0 {
            let end = base.wrapping_add(size - 1);
            let mut inner = self.inner.lock().unwrap();
            if let Some(region) = inner
                .regions
                .iter_mut()
                .find(|r| r.contains(base) && end.wrapping_sub(r.base) < r.size)
            {
                region.perm = perm;
                region.state = state;
                region.kind = kind;
                return Ok(());
            }
        }
        self.map(base, size, perm, state, kind)
    }

    /// unmaps; empties the tables' entries for it (legal from svc, the boundary the ABI names)
    pub fn unmap(&self, base: u32, size: u32) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(index) = inner.regions.iter().position(|r| r.base == base && r.size == size) else {
            return Err(format!("nothing mapped at 0x{base:08X} of {size} bytes"));
        };
        inner.regions.remove(index);
        let remaining = inner.regions.clone();
        for page in u64::from(base / PAGE)..u64::from((base + size) / PAGE) {
            let page = page as u32;
            if !remaining.iter().any(|r| r.covers_page(page)) {
                // safe: empties happen at the svc boundary, the ABI's rule for entry changes
                let tables = unsafe { &mut *self.tables.get() };
                tables.read[page as usize] = std::ptr::null_mut();
                tables.write[page as usize] = std::ptr::null_mut();
                inner.committed.remove(&(page / GROUP_PAGES));
            }
        }
        Ok(())
    }

    /// what region sits at address, as QueryMemory wants it; a gap reports FREE up to the next mapping
    pub fn query(&self, address: u32) -> MemoryInfo {
        let inner = self.inner.lock().unwrap();
        if let Some(region) = inner.regions.iter().find(|r| r.contains(address)) {
            return MemoryInfo {
                base: region.base,
                size: region.size,
                perm: region.perm,
                state: region.state,
            };
        }
        let top = 1u64 << 32;
        let next = inner
            .regions
            .iter()
            .map(|r| r.base)
            .filter(|b| *b > address)
            .min()
            .unwrap_or(top as u32);
        MemoryInfo { base: address, size: next.wrapping_sub(address), perm: Perm::None, state: State::Free }
    }

    /// the watched-write hook (W4 sets one; W2 only counts)
    pub fn set_hook(&self, hook: Option<WatchHook>) {
        self.inner.lock().unwrap().hook = hook;
    }

    /// counters for the boot report
    pub fn stats(&self) -> Stats {
        self.inner.lock().unwrap().stats
    }

    /// mapped regions, base ascending (the boot report's map line)
    pub fn regions(&self) -> Vec<(u32, u32, Perm, State, Kind)> {
        self.inner
            .lock()
            .unwrap()
            .regions
            .iter()
            .map(|r| (r.base, r.size, r.perm, r.state, r.kind))
            .collect()
    }

    fn ensure_commit(&self, address: u32) -> Option<Region> {
        let mut inner = self.inner.lock().unwrap();
        let region = inner.regions.iter().find(|r| r.contains(address)).cloned()?;
        if region.kind == Kind::Mmio {
            inner.stats.mmio_reads += 1;
            return Some(region);
        }
        let group = address / PAGE / GROUP_PAGES;
        if inner.committed.contains(&group) {
            return Some(region);
        }
        let first = group * GROUP_PAGES;
        // safe: the group is inside the reservation; committing makes the pages real
        let group_base = unsafe { self.base.add(first as usize * PAGE as usize) };
        if windows_commit(group_base, (GROUP_PAGES * PAGE) as usize).is_none() {
            inner.stats.faults += 1;
            
            return Some(region);
        }
        for page in first..first + GROUP_PAGES {
            let entry = unsafe { group_base.add((page - first) as usize * PAGE as usize) };
            let tables = unsafe { &mut *self.tables.get() };
            tables.read[page as usize] = entry;
            if region.perm.writable() && region.kind != Kind::Vram {
                tables.write[page as usize] = entry;
            }
        }
        inner.committed.insert(group);
        inner.stats.committed_groups += 1;
        Some(region)
    }

    /// committed and readable, without faulting
    fn readable(&self, address: u32) -> bool {
        !self.read_entry(address / PAGE).is_null()
    }

    fn page_at(&self, address: u32) -> *mut u8 {
        // safe: callers only dereference pages the tables name, which are committed
        unsafe { self.base.add(address as usize) }
    }

    fn region(&self, address: u32) -> Option<Region> {
        self.inner.lock().unwrap().regions.iter().find(|r| r.contains(address)).cloned()
    }

    /// the read8 callback of the Host struct
    pub fn read8(&self, address: u32) -> u8 {
        let Some(region) = self.region(address) else {
            let mut inner = self.inner.lock().unwrap();
            inner.stats.faults += 1;
            inner.stats.last_fault = address;
            return 0;
        };
        if region.kind == Kind::Mmio {
            self.inner.lock().unwrap().stats.mmio_reads += 1;
            return 0;
        }
        if !self.readable(address) {
            self.ensure_commit(address);
            if !self.readable(address) || !region.perm.readable() {
                let mut inner = self.inner.lock().unwrap();
                inner.stats.faults += 1;
                inner.stats.last_fault = address;
                return 0;
            }
        }
        // safe: the tables name committed backing for a readable region
        unsafe { *self.page_at(address) }
    }

    pub fn read16(&self, address: u32) -> u16 {
        u16::from_le_bytes([self.read8(address), self.read8(address.wrapping_add(1))])
    }

    pub fn read32(&self, address: u32) -> u32 {
        u32::from_le_bytes([
            self.read8(address),
            self.read8(address.wrapping_add(1)),
            self.read8(address.wrapping_add(2)),
            self.read8(address.wrapping_add(3)),
        ])
    }

    /// the write8 callback of the Host struct
    pub fn write8(&self, address: u32, value: u8) {
        let Some(region) = self.region(address) else {
            self.inner.lock().unwrap().stats.faults += 1;
            
            return;
        };
        if region.kind == Kind::Mmio {
            self.inner.lock().unwrap().stats.mmio_writes += 1;
            return;
        }
        if !self.readable(address) {
            self.ensure_commit(address);
            if !self.readable(address) {
                self.inner.lock().unwrap().stats.faults += 1;
            
                return;
            }
        }
        if !region.perm.writable() {
            self.inner.lock().unwrap().stats.faults += 1;
            
            return;
        }
        if region.kind == Kind::Vram {
            let call_hook = {
                let mut inner = self.inner.lock().unwrap();
                inner.stats.watched_writes += 1;
                inner.hook.is_some()
            };
            // the hook runs without the lock (it may read memory); taken out and put back, so
            // a hook that re-enters memory from another thread still works
            if call_hook {
                let taken = self.inner.lock().unwrap().hook.take();
                if let Some(hook) = taken.as_ref() {
                    hook(address, value as u64);
                }
                self.inner.lock().unwrap().hook = taken;
            }
        }
        // safe: a writable (or watched) region's backing is committed
        unsafe { *self.page_at(address) = value }
    }

    pub fn write16(&self, address: u32, value: u16) {
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8(address.wrapping_add(i as u32), byte);
        }
    }

    pub fn write32(&self, address: u32, value: u32) {
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8(address.wrapping_add(i as u32), byte);
        }
    }

    /// host-side bulk read (svc handlers, the reports)
    pub fn read_bytes(&self, address: u32, out: &mut [u8]) {
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = self.read8(address.wrapping_add(i as u32));
        }
    }

    /// host-side bulk write (loading the segments, svc handlers)
    pub fn write_bytes(&self, address: u32, bytes: &[u8]) {
        for (i, &byte) in bytes.iter().enumerate() {
            self.write8(address.wrapping_add(i as u32), byte);
        }
    }

    /// the loader's privilege: code lands in read-only regions, this writes the backing the way
    /// the kernel maps a title's segments
    pub fn load_bytes(&self, address: u32, bytes: &[u8]) {
        let end = address.wrapping_add(bytes.len() as u32);
        let mut page = address & !(PAGE - 1);
        while page < end {
            self.ensure_commit(page);
            page = page.wrapping_add(PAGE);
        }
        for (i, &byte) in bytes.iter().enumerate() {
            // safe: the region's backing is committed, the loader is privileged
            unsafe { *self.page_at(address.wrapping_add(i as u32)) = byte };
        }
    }
}

/// reserves bytes of address space, no commit (Windows)
fn windows_alloc(bytes: usize) -> Option<*mut u8> {
    #[cfg(windows)]
    {
        // safe: MEM_RESERVE with PAGE_NOACCESS, nothing readable until committed
        let at = unsafe {
            windows_sys::Win32::System::Memory::VirtualAlloc(
                std::ptr::null(),
                bytes,
                windows_sys::Win32::System::Memory::MEM_RESERVE,
                windows_sys::Win32::System::Memory::PAGE_NOACCESS,
            )
        };
        (!at.is_null()).then_some(at as *mut u8)
    }
    #[cfg(not(windows))]
    {
        let layout = std::alloc::Layout::from_size_align(bytes, 0x10000).ok()?;
        Some(unsafe { std::alloc::alloc_zeroed(layout) })
    }
}

/// commits bytes at an already reserved spot (Windows)
fn windows_commit(at: *mut u8, bytes: usize) -> Option<*mut u8> {
    #[cfg(windows)]
    {
        // safe: at is inside our reservation
        let got = unsafe {
            windows_sys::Win32::System::Memory::VirtualAlloc(
                at as *mut _,
                bytes,
                windows_sys::Win32::System::Memory::MEM_COMMIT,
                windows_sys::Win32::System::Memory::PAGE_READWRITE,
            )
        };
        (!got.is_null()).then_some(got as *mut u8)
    }
    #[cfg(not(windows))]
    {
        let _ = (at, bytes);
        Some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// pages commit lazily a group at a time, the tables fill in, and a write through the tables lands
    #[test]
    fn pages_commit_lazily_and_fill_the_tables() {
        let mem = Memory::new().unwrap();
        mem.map(0x1000_0000, 0x10_0000, Perm::Rw, State::Private, Kind::Heap).unwrap();
        let page = 0x1000_1234 / PAGE;
        assert!(mem.read_entry(page).is_null());
        mem.write8(0x1000_1234, 0xAB);
        let entry = mem.read_entry(page);
        assert!(!entry.is_null());
        // the entry points into the reservation at the page's own offset, 16 pages committed at once
        assert_eq!(unsafe { *entry.add(0x234) }, 0xAB);
        let group = page / GROUP_PAGES;
        for p in group * GROUP_PAGES..(group + 1) * GROUP_PAGES {
            let got = mem.read_entry(p);
            let offset = (i64::from(p) - i64::from(page)) * i64::from(PAGE);
            assert_eq!(got as usize, (entry as i64 + offset) as usize);
            assert_eq!(mem.write_entry(p), got, "a writable region fills both tables");
        }
        assert!(mem.read_entry(page + GROUP_PAGES).is_null(), "only the touched group commits");
        assert_eq!(mem.read8(0x1000_1234), 0xAB);
        mem.write16(0x1000_2000, 0xBEEF);
        assert_eq!(mem.read16(0x1000_2000), 0xBEEF);
        mem.write32(0x1000_3FF0, 0xC0FFEE11);
        assert_eq!(mem.read32(0x1000_3FF0), 0xC0FFEE11);
        assert!(mem.stats().committed_groups >= 1);
    }

    /// an access outside every region faults, answers 0 and counts
    #[test]
    fn an_access_outside_every_region_faults() {
        let mem = Memory::new().unwrap();
        assert_eq!(mem.read32(0x5E00_0000), 0);
        assert_eq!(mem.stats().faults, 4, "read32 walks four bytes");
        mem.write8(0x5E00_0000, 1);
        assert_eq!(mem.stats().faults, 5);
        assert!(mem.read_entry(0x5E00_0000 / PAGE).is_null());
    }

    /// read-only memory fills the read table but not the write table; writes fault
    #[test]
    fn read_only_memory_has_no_write_entries() {
        let mem = Memory::new().unwrap();
        mem.map(0x0010_0000, 0x10_0000, Perm::R, State::Code, Kind::Exe).unwrap();
        mem.write8(0x0010_0000, 0x12);
        assert!(!mem.read_entry(0x0010_0000 / PAGE).is_null());
        assert!(mem.write_entry(0x0010_0000 / PAGE).is_null());
        assert_eq!(mem.stats().faults, 1);
    }

    /// VRAM reads direct but every write arrives at the host (null write table + counters + hook)
    #[test]
    fn vram_writes_arrive_at_the_host() {
        let mem = Memory::new().unwrap();
        mem.map(0x1F00_0000, 0x60_0000, Perm::Rw, State::Locked, Kind::Vram).unwrap();
        let seen = std::sync::Arc::new(AtomicU64::new(0));
        let hook_seen = seen.clone();
        mem.set_hook(Some(Box::new(move |address, value| {
            if address == 0x1F00_0041 && value == 0x55 {
                hook_seen.fetch_add(1, Ordering::Relaxed);
            }
        })));
        mem.write32(0x1F00_0040, 0x0000_5500);
        assert_eq!(seen.load(Ordering::Relaxed), 1, "the hook saw the byte carrying 0x55");
        assert!(mem.write_entry(0x1F00_0040 / PAGE).is_null());
        assert!(!mem.read_entry(0x1F00_0040 / PAGE).is_null());
        assert_eq!(mem.read32(0x1F00_0040), 0x0000_5500);
        assert_eq!(mem.stats().watched_writes, 4);
    }

    /// MMIO reads and writes never commit and answer 0
    #[test]
    fn mmio_answers_zero_without_backing() {
        let mem = Memory::new().unwrap();
        mem.map(0x1EC0_0000, 0x40_0000, Perm::Rw, State::Io, Kind::Mmio).unwrap();
        assert_eq!(mem.read8(0x1EC0_1234), 0);
        mem.write8(0x1EC0_1234, 9);
        assert_eq!(mem.stats().mmio_reads, 1);
        assert_eq!(mem.stats().mmio_writes, 1);
        assert!(mem.read_entry(0x1EC0_1234 / PAGE).is_null());
    }

    /// unmap empties the tables' entries for the range and keeps overlapping regions' entries
    #[test]
    fn unmap_empties_the_entries() {
        let mem = Memory::new().unwrap();
        mem.map(0x2000_0000, 0x10_0000, Perm::Rw, State::Private, Kind::Heap).unwrap();
        mem.write8(0x2000_0000, 1);
        assert!(!mem.read_entry(0x2000_0000 / PAGE).is_null());
        mem.unmap(0x2000_0000, 0x10_0000).unwrap();
        assert!(mem.read_entry(0x2000_0000 / PAGE).is_null());
        assert!(mem.write_entry(0x2000_0000 / PAGE).is_null());
        assert_eq!(mem.read8(0x2000_0000), 0);
        assert_eq!(mem.stats().faults, 1);
        assert!(mem.unmap(0x2000_0000, 0x10_0000).is_err());
    }

    /// query answers regions and the free runs between them, as the kernel reports them
    #[test]
    fn query_reports_regions_and_free_runs() {
        let mem = Memory::new().unwrap();
        mem.map(0x0010_0000, 0x40_5000, Perm::R, State::Code, Kind::Exe).unwrap();
        mem.map(0x0800_0000, 0x40_0000, Perm::Rw, State::Private, Kind::Heap).unwrap();
        let code = mem.query(0x0010_0100);
        assert_eq!(code, MemoryInfo { base: 0x0010_0000, size: 0x40_5000, perm: Perm::R, state: State::Code });
        let gap = mem.query(0x0060_0000);
        assert_eq!(gap, MemoryInfo { base: 0x0060_0000, size: 0x0800_0000 - 0x0060_0000, perm: Perm::None, state: State::Free });
        assert_eq!(mem.query(0x8000_0000).state, State::Free);
    }

    /// a region map over a live mapping is refused, and misaligned ones too
    #[test]
    fn bad_maps_are_refused() {
        let mem = Memory::new().unwrap();
        mem.map(0x1000_0000, 0x10_0000, Perm::Rw, State::Private, Kind::Heap).unwrap();
        assert!(mem.map(0x100F_F000, 0x2_0000, Perm::Rw, State::Private, Kind::Heap).is_err());
        assert!(mem.map(0x1100_0123, 0x10_0000, Perm::Rw, State::Private, Kind::Heap).is_err());
        assert!(mem.map(0x1100_0000, 0x123, Perm::Rw, State::Private, Kind::Heap).is_err());
    }

    /// bulk writes land whole (the segment loader uses them)
    #[test]
    fn bulk_writes_land_whole() {
        let mem = Memory::new().unwrap();
        mem.map(0x0010_0000, 0x1_0000, Perm::Rw, State::Code, Kind::Exe).unwrap();
        let bytes: Vec<u8> = (0..0x1234u32).map(|i| i as u8).collect();
        mem.write_bytes(0x0010_0FF0, &bytes);
        for (i, &byte) in bytes.iter().enumerate() {
            assert_eq!(mem.read8(0x0010_0FF0 + i as u32), byte);
        }
    }
}

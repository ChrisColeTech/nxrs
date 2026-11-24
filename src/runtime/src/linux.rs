//! The Linux platform layer (x86-64): what `mem.rs`, `fault.rs`, `fiber.rs`, `profile.rs` and `process.rs` do with
//! Win32 on Windows.
//!
//! - **Memory.** Reservations are `PROT_NONE` `MAP_NORESERVE` mappings at fixed addresses (`MAP_FIXED_NOREPLACE`);
//!   commit is `mprotect` read-write, decommit `MADV_DONTNEED` + `PROT_NONE` (private anonymous pages read zero
//!   afterwards, as decommitted pages do on Windows). Linux keeps no committed / reserved distinction, so one byte
//!   per guest page (a lazily populated map over the 39-bit space) records it, and a table of reservations stands
//!   for VirtualQuery's allocation base.
//! - **Write watch** (GetWriteWatch): a userfaultfd in asynchronous write-protect mode (Linux 6.7) over the watched
//!   reservations. The kernel lifts a page's protection itself at the first write (kernel-mode writes such as
//!   `read()` into guest memory included, with no round trip to us), and PAGEMAP_SCAN lists the pages whose
//!   protection was lifted and protects them again. Without it (an older kernel) `written_pages` answers None and
//!   the callers fall back to comparing snapshots, as for memory without a watch on Windows.
//! - **Faults** (vectored exception handlers): one handler for SIGSEGV, SIGBUS, SIGILL, SIGFPE and SIGTRAP passes
//!   each fault to the hooks the parts registered (`add_fault_hook`, newest first, as `AddVectoredExceptionHandler(1,
//!   ..)` orders them); a fault no hook takes goes to the handler that was installed before (Rust's stack-overflow
//!   report) or gets the default action.
//! - **Thread samples** (SuspendThread + GetThreadContext): the thread is sent SIGPROF and its handler copies the
//!   interrupted pc out of the signal context.
//! - **Memory cap** (a job object): a cgroup v2 of the process's own with `memory.max` where the user's cgroup tree
//!   is delegated (systemd user sessions), else a watchdog thread on the resident size.

use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

pub const PAGE: u64 = 0x1000;
const PS_LIMIT: u64 = 1 << 39;

// ---------------------------------------------------------------- memory

/// Page states in the map: free, reserved, committed with a protection.
const PS_FREE: u8 = 0;
const PS_RESERVED: u8 = 1;
const PS_NONE: u8 = 2;
#[allow(dead_code)]
const PS_READ: u8 = 3;
const PS_RW: u8 = 4;

static PSTATE: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
/// Reservations made through `reserve` (base -> size): VirtualQuery's allocation base.
static RESERVATIONS: Mutex<std::collections::BTreeMap<u64, u64>> =
    Mutex::new(std::collections::BTreeMap::new());
/// Page states of reservations above the map's 39 bits (host tables, tests): one byte a page, per reservation.
static HIGH: Mutex<std::collections::BTreeMap<u64, Vec<u8>>> =
    Mutex::new(std::collections::BTreeMap::new());
/// The userfaultfd (write watch) and /proc/self/pagemap: -1 while closed, -2 when write watch is unavailable.
static WW_FD: AtomicI32 = AtomicI32::new(-1);
static PM_FD: AtomicI32 = AtomicI32::new(-1);
static WW_LOCK: Mutex<()> = Mutex::new(());
/// Bases of the reservations under the write watch.
static WATCHED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn pstate() -> *mut u8 {
    let p = PSTATE.load(Ordering::Acquire);
    if !p.is_null() {
        return p;
    }
    // SAFETY: an anonymous mapping of our own; only the pages written cost memory
    let m = unsafe {
        libc::mmap(
            core::ptr::null_mut(),
            (PS_LIMIT / PAGE) as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if m == libc::MAP_FAILED {
        crate::rt::fatal("memory: the page state map could not be reserved");
    }
    match PSTATE.compare_exchange(
        core::ptr::null_mut(),
        m as *mut u8,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => m as *mut u8,
        Err(other) => {
            // SAFETY: our own mapping, never published
            unsafe { libc::munmap(m, (PS_LIMIT / PAGE) as usize) };
            other
        }
    }
}

fn ps_set(addr: u64, n: u64, v: u8) {
    if n == 0 {
        return;
    }
    if addr >= PS_LIMIT {
        let mut h = HIGH.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((&base, st)) = h.range_mut(..=addr).next_back() {
            let (i, e) = (
                ((addr - base) / PAGE) as usize,
                ((addr + n - base).div_ceil(PAGE) as usize).min(st.len()),
            );
            if i < e {
                st[i..e].fill(v);
            }
        }
        return;
    }
    let end = (addr + n).min(PS_LIMIT);
    let (a, b) = (addr / PAGE, end.div_ceil(PAGE));
    // SAFETY: inside the map
    unsafe { core::ptr::write_bytes(pstate().add(a as usize), v, (b - a) as usize) };
}

fn ps_get(addr: u64) -> u8 {
    if addr >= PS_LIMIT {
        let h = HIGH.lock().unwrap_or_else(|p| p.into_inner());
        return match h.range(..=addr).next_back() {
            Some((&base, st)) => st
                .get(((addr - base) / PAGE) as usize)
                .copied()
                .unwrap_or(PS_FREE),
            None => PS_FREE,
        };
    }
    // SAFETY: inside the map
    unsafe { *pstate().add((addr / PAGE) as usize) }
}

/// A reservation's bookkeeping (its page states, when it lies above the map).
fn track(addr: u64, n: u64) {
    RESERVATIONS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(addr, n);
    if addr >= PS_LIMIT {
        HIGH.lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(addr, vec![PS_RESERVED; n.div_ceil(PAGE) as usize]);
    } else {
        ps_set(addr, n, PS_RESERVED);
    }
}

/// Opens the write watch (once): a userfaultfd in asynchronous write-protect mode and /proc/self/pagemap.
fn ww_open() -> Option<i32> {
    let fd = WW_FD.load(Ordering::Acquire);
    if fd >= 0 {
        return Some(fd);
    }
    if fd == -2 {
        return None;
    }
    let _g = WW_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fd = WW_FD.load(Ordering::Acquire);
    if fd != -1 {
        return (fd >= 0).then_some(fd);
    }
    const UFFD_USER_MODE_ONLY: libc::c_long = 1;
    // SAFETY: plain syscalls on descriptors we own
    unsafe {
        let fd = libc::syscall(
            libc::SYS_userfaultfd,
            (libc::O_CLOEXEC | libc::O_NONBLOCK) as libc::c_long | UFFD_USER_MODE_ONLY,
        ) as i32;
        let mut api = UffdioApi {
            api: UFFD_API,
            features: UFFD_FEATURE_WP_ASYNC | UFFD_FEATURE_WP_UNPOPULATED,
            ioctls: 0,
        };
        if fd < 0 || libc::ioctl(fd, UFFDIO_API, &mut api) != 0 {
            let e = std::io::Error::last_os_error();
            crate::rt::log(&format!("memory: no asynchronous userfaultfd write protection ({e}): written guest memory is found by comparison instead"));
            if fd >= 0 {
                libc::close(fd);
            }
            WW_FD.store(-2, Ordering::Release);
            return None;
        }
        let pm = libc::open(
            c"/proc/self/pagemap".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        );
        if pm < 0 {
            libc::close(fd);
            WW_FD.store(-2, Ordering::Release);
            return None;
        }
        PM_FD.store(pm, Ordering::Release);
        WW_FD.store(fd, Ordering::Release);
        Some(fd)
    }
}

#[repr(C)]
struct UffdioApi {
    api: u64,
    features: u64,
    ioctls: u64,
}
#[repr(C)]
struct UffdioRegister {
    start: u64,
    len: u64,
    mode: u64,
    ioctls: u64,
}
const UFFD_API: u64 = 0xAA;
const UFFD_FEATURE_WP_UNPOPULATED: u64 = 1 << 13;
const UFFD_FEATURE_WP_ASYNC: u64 = 1 << 15;
const UFFDIO_REGISTER_MODE_WP: u64 = 1 << 1;
// _IOWR(0xAA, 0x3F, struct uffdio_api) and _IOWR(0xAA, 0x00, struct uffdio_register)
const UFFDIO_API: libc::c_ulong = 0xC018_AA3F;
const UFFDIO_REGISTER: libc::c_ulong = 0xC020_AA00;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct PageRegion {
    start: u64,
    end: u64,
    categories: u64,
}
#[repr(C)]
#[derive(Default)]
struct PmScanArg {
    size: u64,
    flags: u64,
    start: u64,
    end: u64,
    walk_end: u64,
    vec: u64,
    vec_len: u64,
    max_pages: u64,
    category_inverted: u64,
    category_mask: u64,
    category_anyof_mask: u64,
    return_mask: u64,
}
// _IOWR('f', 16, struct pm_scan_arg)
const PAGEMAP_SCAN: libc::c_ulong = 0xC060_6610;
const PAGE_IS_WRITTEN: u64 = 1 << 1;
const PAGE_IS_PRESENT: u64 = 1 << 3;
const PAGE_IS_SWAPPED: u64 = 1 << 4;
const PAGE_IS_PFNZERO: u64 = 1 << 5;
const PM_SCAN_WP_MATCHING: u64 = 1 << 0;
const PM_SCAN_CHECK_WPASYNC: u64 = 1 << 1;

fn watch_register(addr: u64, n: u64) {
    let Some(fd) = ww_open() else { return };
    // Transparent huge pages would populate (and count as written) 2 MiB at the first write of one page: writes are
    // watched per 4 KiB page, as on Windows.
    // SAFETY: our own reservation
    unsafe { libc::madvise(addr as *mut _, n as usize, libc::MADV_NOHUGEPAGE) };
    let mut reg = UffdioRegister {
        start: addr,
        len: n,
        mode: UFFDIO_REGISTER_MODE_WP,
        ioctls: 0,
    };
    // SAFETY: a registration of our own range
    if unsafe { libc::ioctl(fd, UFFDIO_REGISTER, &mut reg) } != 0 {
        crate::rt::log(&format!(
            "memory: write watch over {addr:#x}+{n:#x} failed ({})",
            std::io::Error::last_os_error()
        ));
    } else {
        WATCHED.lock().unwrap_or_else(|p| p.into_inner()).push(addr);
    }
}

/// Reserves [addr, addr+n) exactly there (no access), optionally write-watched.
pub fn reserve(addr: u64, n: u64, watch: bool) -> bool {
    pstate();
    // SAFETY: MAP_FIXED_NOREPLACE never takes over an existing mapping
    let p = unsafe {
        libc::mmap(
            addr as *mut _,
            n as usize,
            libc::PROT_NONE,
            libc::MAP_PRIVATE
                | libc::MAP_ANONYMOUS
                | libc::MAP_NORESERVE
                | libc::MAP_FIXED_NOREPLACE,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return false;
    }
    if p as u64 != addr {
        // a kernel without MAP_FIXED_NOREPLACE put it elsewhere
        // SAFETY: the mapping just made
        unsafe { libc::munmap(p, n as usize) };
        return false;
    }
    track(addr, n);
    if watch {
        watch_register(addr, n);
    }
    true
}

/// Reserves n bytes wherever the kernel likes, optionally write-watched.
pub fn reserve_anywhere(n: u64, watch: bool) -> Option<u64> {
    pstate();
    // SAFETY: a fresh anonymous mapping
    let p = unsafe {
        libc::mmap(
            core::ptr::null_mut(),
            n as usize,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return None;
    }
    let a = p as u64;
    track(a, n);
    if watch {
        watch_register(a, n);
    }
    Some(a)
}

pub fn release(addr: u64) {
    WATCHED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|&b| b != addr);
    let n = RESERVATIONS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&addr);
    if let Some(n) = n {
        // SAFETY: a whole reservation of ours
        unsafe { libc::munmap(addr as *mut _, n as usize) };
        if addr >= PS_LIMIT {
            HIGH.lock().unwrap_or_else(|p| p.into_inner()).remove(&addr);
        } else {
            ps_set(addr, n, PS_FREE);
        }
    }
}

pub fn commit(addr: u64, n: u64) -> bool {
    if n == 0 {
        return true;
    }
    // SAFETY: inside a reservation of ours
    let ok = unsafe {
        libc::mprotect(
            addr as *mut _,
            n as usize,
            libc::PROT_READ | libc::PROT_WRITE,
        )
    } == 0;
    if ok {
        ps_set(addr, n, PS_RW);
    }
    ok
}

pub fn decommit(addr: u64, n: u64) -> bool {
    if n == 0 {
        return true;
    }
    // SAFETY: inside a reservation of ours
    let ok = unsafe {
        libc::madvise(addr as *mut _, n as usize, libc::MADV_DONTNEED) == 0
            && libc::mprotect(addr as *mut _, n as usize, libc::PROT_NONE) == 0
    };
    if ok {
        ps_set(addr, n, PS_RESERVED);
    }
    ok
}

/// 0 no access, 1 read, 2 read-write (`mem::Prot` in that order).
pub fn protect(addr: u64, n: u64, prot: u8) -> bool {
    if n == 0 {
        return true;
    }
    let p = match prot {
        0 => libc::PROT_NONE,
        1 => libc::PROT_READ,
        _ => libc::PROT_READ | libc::PROT_WRITE,
    };
    // SAFETY: inside a mapping of ours
    let ok = unsafe { libc::mprotect(addr as *mut _, n as usize, p) } == 0;
    if ok {
        ps_set(addr, n, PS_NONE + prot.min(2));
    }
    ok
}

/// Zeroed host memory (populated at first touch), for the runtime's own tables.
pub fn host_alloc(n: u64) -> Option<*mut u8> {
    // SAFETY: a fresh anonymous mapping
    let p = unsafe {
        libc::mmap(
            core::ptr::null_mut(),
            n as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    (p != libc::MAP_FAILED).then_some(p as *mut u8)
}

/// Copies [addr, addr+n) into `out` without faulting: false where part of it cannot be read.
pub fn read_nofault(addr: u64, out: &mut [u8]) -> bool {
    if out.is_empty() {
        return true;
    }
    let local = libc::iovec {
        iov_base: out.as_mut_ptr() as *mut _,
        iov_len: out.len(),
    };
    let remote = libc::iovec {
        iov_base: addr as *mut _,
        iov_len: out.len(),
    };
    // SAFETY: process_vm_readv on our own process reads through the page tables and fails instead of faulting
    unsafe {
        libc::process_vm_readv(libc::getpid(), &local, 1, &remote, 1, 0) == out.len() as isize
    }
}

/// Every byte of [addr, addr+n) is mapped and readable.
pub fn readable(addr: u64, n: u64) -> bool {
    let end = addr.saturating_add(n.max(1));
    let mut a = addr;
    let mut b = [0u8; 1];
    while a < end {
        // pages this layer committed no-access are known without a syscall
        if a < PS_LIMIT && matches!(ps_get(a), PS_RESERVED | PS_NONE) {
            return false;
        }
        if !read_nofault(a, &mut b) {
            return false;
        }
        a = (a | (PAGE - 1)) + 1;
    }
    true
}

/// Committed bytes of [a, a+n) (any protection).
pub fn committed_in(a: u64, n: u64) -> u64 {
    if a >= PS_LIMIT {
        return committed_runs(a, n).iter().map(|r| r.1).sum();
    }
    let end = (a + n).min(PS_LIMIT);
    let p = pstate();
    let mut total = 0u64;
    let mut i = a / PAGE;
    let e = end.div_ceil(PAGE);
    while i < e {
        // SAFETY: inside the map; only the written part of it is populated, the rest reads zero (PS_FREE)
        if unsafe { *p.add(i as usize) } >= PS_NONE {
            total += PAGE;
        }
        i += 1;
    }
    total
}

/// The committed runs of [a, a+n): (start, length) pairs.
pub fn committed_runs(a: u64, n: u64) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    let end = if a >= PS_LIMIT {
        a + n
    } else {
        (a + n).min(PS_LIMIT)
    };
    let mut p = a & !(PAGE - 1);
    while p < end {
        let next = (p + PAGE).min(end);
        if ps_get(p) >= PS_NONE {
            let s = p.max(a);
            match out.last_mut() {
                Some(l) if l.0 + l.1 == s => l.1 += next - s,
                _ => out.push((s, next - s)),
            }
        }
        p = next;
    }
    out
}

/// What VirtualQuery says of one page: (committed, protection 0 none / 1 read / 2 read-write, the reservation's base
/// and size). None outside every reservation of this layer.
pub fn query(addr: u64) -> Option<(bool, u8, u64, u64)> {
    let r = RESERVATIONS.lock().unwrap_or_else(|p| p.into_inner());
    let (&base, &size) = r.range(..=addr).next_back()?;
    if addr >= base + size {
        return None;
    }
    let st = ps_get(addr);
    Some((st >= PS_NONE, st.saturating_sub(PS_NONE), base, size))
}

/// Every page of [a, a+n) is committed read-write in a reservation of this layer.
pub fn writable(a: u64, n: u64) -> bool {
    let end = a.saturating_add(n.max(1));
    let mut p = a & !(PAGE - 1);
    while p < end {
        match query(p) {
            Some((true, 2, _, _)) => {}
            _ => return false,
        }
        p += PAGE;
    }
    true
}

/// Sets the process's CPU affinity to the first `n` CPUs: every thread it has now, and (inherited) the threads they
/// start.
pub fn limit_cpus(n: u32) -> bool {
    // SAFETY: a cpu_set_t of our own, applied to our own threads
    unsafe {
        let mut set: libc::cpu_set_t = core::mem::zeroed();
        for i in 0..n as usize {
            libc::CPU_SET(i, &mut set);
        }
        let mut ok = libc::sched_setaffinity(0, core::mem::size_of::<libc::cpu_set_t>(), &set) == 0;
        if let Ok(rd) = std::fs::read_dir("/proc/self/task") {
            for t in rd.flatten() {
                if let Some(tid) = t.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) {
                    ok &=
                        libc::sched_setaffinity(tid, core::mem::size_of::<libc::cpu_set_t>(), &set)
                            == 0;
                }
            }
        }
        ok
    }
}

/// Whether `addr` lies in a reservation under the write watch.
pub fn watched(addr: u64) -> bool {
    let base = allocation_of(addr, addr + 1);
    base != 0
        && WATCHED
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&base)
}

/// The base of the reservation holding all of [lo, hi), 0 when none does.
pub fn allocation_of(lo: u64, hi: u64) -> u64 {
    let r = RESERVATIONS.lock().unwrap_or_else(|p| p.into_inner());
    match r.range(..=lo).next_back() {
        Some((&b, &s)) if hi <= b + s => b,
        _ => 0,
    }
}

/// The pages of [addr, addr+n) written since the last reset, None without write watch. `reset` protects them again.
pub fn written_pages(addr: u64, n: u64, reset: bool) -> Option<Vec<u64>> {
    ww_open()?;
    let fd = PM_FD.load(Ordering::Acquire);
    let mut out = Vec::new();
    let mut r = [PageRegion::default(); 256];
    let (mut at, end) = (addr & !(PAGE - 1), addr + n);
    while at < end {
        // Written: protection lifted, on a populated page (present or swapped out) that is not the shared zero page.
        // A page never touched since its commit has no protection marker either; a read maps the zero page there.
        // Neither is a write, as on Windows.
        let mut arg = PmScanArg {
            size: core::mem::size_of::<PmScanArg>() as u64,
            flags: if reset {
                PM_SCAN_WP_MATCHING | PM_SCAN_CHECK_WPASYNC
            } else {
                0
            },
            start: at,
            end,
            vec: r.as_mut_ptr() as u64,
            vec_len: r.len() as u64,
            category_inverted: PAGE_IS_PFNZERO,
            category_mask: PAGE_IS_WRITTEN | PAGE_IS_PFNZERO,
            category_anyof_mask: PAGE_IS_PRESENT | PAGE_IS_SWAPPED,
            return_mask: PAGE_IS_WRITTEN,
            ..Default::default()
        };
        // SAFETY: the scan writes at most vec_len regions into `r`
        let k = unsafe { libc::ioctl(fd, PAGEMAP_SCAN, &mut arg) };
        if k < 0 {
            return None;
        }
        for reg in &r[..k as usize] {
            let mut p = reg.start;
            while p < reg.end {
                out.push(p);
                p += PAGE;
            }
        }
        if arg.walk_end <= at {
            break;
        }
        at = arg.walk_end;
    }
    Some(out)
}

// ---------------------------------------------------------------- faults

/// One synchronous fault, as a hook sees it.
pub struct Fault {
    pub sig: i32,
    /// The faulting data address (SIGSEGV / SIGBUS).
    pub addr: u64,
    /// The access was a write (x86's page-fault error code bit 1; AArch64: ESR's WnR of a data abort).
    pub write: bool,
    /// The access was an instruction fetch.
    pub exec: bool,
    uc: *mut libc::ucontext_t,
}

/// Whether a fault hook can single-step (`Fault::set_trap`): x86-64's trap flag. AArch64 Linux has no user-mode single
/// step outside ptrace (MDSCR_EL1.SS is the kernel's), so there `set_trap` does nothing.
pub const CAN_SINGLE_STEP: bool = cfg!(target_arch = "x86_64");

#[cfg(target_arch = "x86_64")]
impl Fault {
    pub fn rip(&self) -> u64 {
        // SAFETY: the kernel's signal frame
        unsafe { (*self.uc).uc_mcontext.gregs[libc::REG_RIP as usize] as u64 }
    }
    pub fn rsp(&self) -> u64 {
        // SAFETY: as above
        unsafe { (*self.uc).uc_mcontext.gregs[libc::REG_RSP as usize] as u64 }
    }
    pub fn rbp(&self) -> u64 {
        // SAFETY: as above
        unsafe { (*self.uc).uc_mcontext.gregs[libc::REG_RBP as usize] as u64 }
    }
    /// The trap flag: a SIGTRAP after the next instruction (single step).
    pub fn set_trap(&mut self, on: bool) {
        // SAFETY: as above; the kernel restores EFLAGS from the frame
        unsafe {
            let f = &mut (*self.uc).uc_mcontext.gregs[libc::REG_EFL as usize];
            if on {
                *f |= 0x100;
            } else {
                *f &= !0x100;
            }
        }
    }
}

/// AArch64: the same accessors over the kernel's `struct sigcontext` (pc, sp, x29 the frame pointer).
#[cfg(target_arch = "aarch64")]
impl Fault {
    pub fn rip(&self) -> u64 {
        // SAFETY: the kernel's signal frame
        unsafe { (*self.uc).uc_mcontext.pc }
    }
    pub fn rsp(&self) -> u64 {
        // SAFETY: as above
        unsafe { (*self.uc).uc_mcontext.sp }
    }
    pub fn rbp(&self) -> u64 {
        // SAFETY: as above
        unsafe { (*self.uc).uc_mcontext.regs[29] }
    }
    /// Nothing on AArch64 (`CAN_SINGLE_STEP`).
    pub fn set_trap(&mut self, _on: bool) {}
}

/// The fault's access kind from the signal frame: (write, instruction fetch).
#[cfg(target_arch = "x86_64")]
unsafe fn access_kind(sig: i32, ucp: *const libc::ucontext_t) -> (bool, bool) {
    let err = (*ucp).uc_mcontext.gregs[libc::REG_ERR as usize] as u64;
    (
        sig == libc::SIGSEGV && err & 2 != 0,
        sig == libc::SIGSEGV && err & 16 != 0,
    )
}

/// AArch64: from the ESR record the kernel puts in the frame's `__reserved` area (`esr_context`).
#[cfg(target_arch = "aarch64")]
unsafe fn access_kind(sig: i32, ucp: *const libc::ucontext_t) -> (bool, bool) {
    if sig != libc::SIGSEGV {
        return (false, false);
    }
    // `__reserved` follows pstate, 16-aligned (offset 288 of the sigcontext); libc keeps the field private
    let mc = core::ptr::addr_of!((*ucp).uc_mcontext);
    let res = (core::ptr::addr_of!((*mc).pstate) as usize + 8 + 15) & !15;
    let area = core::slice::from_raw_parts(res as *const u8, 4096);
    match find_esr(area) {
        Some(esr) => esr_access(esr),
        None => (false, false),
    }
}

/// The ESR value in a sigcontext `__reserved` area: its records are {u32 magic, u32 size, payload}, up to a zero magic.
#[cfg(any(target_arch = "aarch64", test))]
fn find_esr(area: &[u8]) -> Option<u64> {
    const ESR_MAGIC: u32 = 0x4553_5201;
    let mut at = 0;
    while at + 16 <= area.len() {
        let magic = u32::from_ne_bytes(area[at..at + 4].try_into().ok()?);
        let size = u32::from_ne_bytes(area[at + 4..at + 8].try_into().ok()?) as usize;
        if magic == 0 || size < 8 || at + size > area.len() {
            return None;
        }
        if magic == ESR_MAGIC {
            return Some(u64::from_ne_bytes(area[at + 8..at + 16].try_into().ok()?));
        }
        at += size;
    }
    None
}

/// (write, instruction fetch) of an abort's ESR: an instruction abort (EC 0x20 / 0x21) is a fetch; a data abort
/// (EC 0x24 / 0x25) is a write when WnR (bit 6) is set and it is not a cache maintenance operation (CM, bit 8).
#[cfg(any(target_arch = "aarch64", test))]
fn esr_access(esr: u64) -> (bool, bool) {
    match (esr >> 26) & 0x3f {
        0x20 | 0x21 => (false, true),
        0x24 | 0x25 => (esr & (1 << 6) != 0 && esr & (1 << 8) == 0, false),
        _ => (false, false),
    }
}

/// The interrupted pc of a signal frame.
unsafe fn frame_pc(ucp: *const libc::ucontext_t) -> u64 {
    #[cfg(target_arch = "x86_64")]
    return (*ucp).uc_mcontext.gregs[libc::REG_RIP as usize] as u64;
    #[cfg(target_arch = "aarch64")]
    return (*ucp).uc_mcontext.pc;
}

impl Fault {
    pub fn what(&self) -> &'static str {
        match self.sig {
            libc::SIGSEGV => "access violation",
            libc::SIGBUS => "bus error",
            libc::SIGILL => "illegal instruction",
            libc::SIGFPE => "arithmetic exception (integer division by zero)",
            libc::SIGTRAP => "breakpoint / single step",
            _ => "signal",
        }
    }
}

/// A fault hook: true when it dealt with the fault (execution goes on at the faulting instruction, or wherever the
/// hook moved it), false to pass it on.
pub type FaultHook = unsafe fn(&mut Fault) -> bool;

const MAX_HOOKS: usize = 16;
static HOOKS: [AtomicUsize; MAX_HOOKS] = [const { AtomicUsize::new(0) }; MAX_HOOKS];
static NHOOKS: AtomicUsize = AtomicUsize::new(0);
static OLD: Mutex<Vec<(i32, libc::sigaction)>> = Mutex::new(Vec::new());
static OLD_TABLE: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
static OLD_FLAGS: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

/// Adds a hook; hooks run newest first (Windows' AddVectoredExceptionHandler(1, ..) order).
pub fn add_fault_hook(h: FaultHook) {
    install_handler();
    let i = NHOOKS.fetch_add(1, Ordering::AcqRel);
    if i >= MAX_HOOKS {
        crate::rt::fatal("faults: too many fault hooks");
    }
    HOOKS[i].store(h as usize, Ordering::Release);
}

unsafe extern "C" fn handler(sig: i32, si: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    let errno = *libc::__errno_location();
    let ucp = uc as *mut libc::ucontext_t;
    let addr = if sig == libc::SIGSEGV || sig == libc::SIGBUS {
        (*si).si_addr() as u64
    } else {
        0
    };
    let (write, exec) = access_kind(sig, ucp);
    let mut f = Fault {
        sig,
        addr,
        write,
        exec,
        uc: ucp,
    };
    let n = NHOOKS.load(Ordering::Acquire).min(MAX_HOOKS);
    for i in (0..n).rev() {
        let h = HOOKS[i].load(Ordering::Acquire);
        if h != 0 && core::mem::transmute::<usize, FaultHook>(h)(&mut f) {
            *libc::__errno_location() = errno;
            return;
        }
    }
    // nobody's: the handler that was there before (Rust's stack overflow report), else the default action when the
    // instruction runs again
    let old = OLD_TABLE[sig as usize & 31].load(Ordering::Acquire) as usize;
    let flags = OLD_FLAGS[sig as usize & 31].load(Ordering::Acquire) as i32;
    if old > 1 {
        if flags & libc::SA_SIGINFO != 0 {
            core::mem::transmute::<
                usize,
                unsafe extern "C" fn(i32, *mut libc::siginfo_t, *mut libc::c_void),
            >(old)(sig, si, uc);
        } else {
            core::mem::transmute::<usize, unsafe extern "C" fn(i32)>(old)(sig);
        }
        return;
    }
    libc::signal(sig, libc::SIG_DFL);
    if sig == libc::SIGTRAP {
        libc::raise(sig);
    }
}

fn install_handler() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        alt_stack();
        for sig in [
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
            libc::SIGTRAP,
        ] {
            // SAFETY: a process-wide handler of ours; the old one is kept and chained to
            unsafe {
                let mut sa: libc::sigaction = core::mem::zeroed();
                sa.sa_sigaction = handler as *const () as usize;
                sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK | libc::SA_NODEFER;
                libc::sigemptyset(&mut sa.sa_mask);
                let mut old: libc::sigaction = core::mem::zeroed();
                libc::sigaction(sig, &sa, &mut old);
                OLD_TABLE[sig as usize & 31].store(old.sa_sigaction as u64, Ordering::Release);
                OLD_FLAGS[sig as usize & 31].store(old.sa_flags as u64, Ordering::Release);
                OLD.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push((sig, old));
            }
        }
    });
}

/// A signal stack for the calling thread (a fault from a thread whose stack overflowed still gets reported). Rust's
/// own threads have one already; this is for the rest, and idempotent.
pub fn alt_stack() {
    // SAFETY: sigaltstack on the calling thread with a leaked stack of its own
    unsafe {
        let mut cur: libc::stack_t = core::mem::zeroed();
        libc::sigaltstack(core::ptr::null(), &mut cur);
        if cur.ss_flags & libc::SS_DISABLE == 0 && !cur.ss_sp.is_null() {
            return;
        }
        let size = 256 << 10;
        let p = libc::mmap(
            core::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if p == libc::MAP_FAILED {
            return;
        }
        let ss = libc::stack_t {
            ss_sp: p,
            ss_flags: 0,
            ss_size: size,
        };
        libc::sigaltstack(&ss, core::ptr::null_mut());
    }
}

// ---------------------------------------------------------------- modules

/// The loaded image holding `a`: (its file name, its load base), None outside every image.
pub fn module_of(a: u64) -> Option<(String, u64)> {
    let mut di: libc::Dl_info = unsafe { core::mem::zeroed() };
    // SAFETY: dladdr only reads the loader's tables
    if unsafe { libc::dladdr(a as *const _, &mut di) } == 0 || di.dli_fname.is_null() {
        return None;
    }
    // SAFETY: a C string of the loader's
    let name = unsafe { std::ffi::CStr::from_ptr(di.dli_fname) }.to_string_lossy();
    let name = name.rsplit('/').next().unwrap_or(&name).to_string();
    Some((name, di.dli_fbase as u64))
}

extern "C" {
    fn backtrace(buf: *mut *mut libc::c_void, n: libc::c_int) -> libc::c_int;
}

/// The return addresses of the calling thread's stack (glibc's unwinder over the images' .eh_frame; from a signal
/// handler it passes through the signal frame into the interrupted code).
pub fn host_backtrace(max: usize) -> Vec<u64> {
    let mut v = vec![core::ptr::null_mut(); max.max(1)];
    // SAFETY: glibc writes at most `max` entries
    let n = unsafe { backtrace(v.as_mut_ptr(), v.len() as i32) }.max(0) as usize;
    v[..n].iter().map(|&p| p as u64).collect()
}

// ---------------------------------------------------------------- threads

/// A host thread other threads can sample: its pthread.
#[derive(Clone, Copy)]
pub struct HostThread(pub libc::pthread_t);
// SAFETY: a pthread_t is a plain id, usable from any thread
unsafe impl Send for HostThread {}

pub fn current_thread() -> HostThread {
    // SAFETY: pthread_self has no preconditions
    HostThread(unsafe { libc::pthread_self() })
}

static SAMPLE_PC: AtomicU64 = AtomicU64::new(0);
static SAMPLE_LOCK: Mutex<()> = Mutex::new(());
static SAMPLE_SEM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

unsafe extern "C" fn sample_handler(_s: i32, _si: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    let u = uc as *mut libc::ucontext_t;
    SAMPLE_PC.store(frame_pc(u), Ordering::Release);
    if let Some(&s) = SAMPLE_SEM.get() {
        libc::sem_post(s as *mut libc::sem_t);
    }
}

/// The host pc a thread runs at now (it is interrupted by SIGPROF for that instant): the profiler's sample.
pub fn thread_pc(t: HostThread) -> Option<u64> {
    let _g = SAMPLE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let sem = *SAMPLE_SEM.get_or_init(|| {
        // SAFETY: a leaked semaphore and a process-wide SIGPROF handler of ours
        unsafe {
            let s = Box::into_raw(Box::new(core::mem::zeroed::<libc::sem_t>()));
            libc::sem_init(s, 0, 0);
            let mut sa: libc::sigaction = core::mem::zeroed();
            sa.sa_sigaction = sample_handler as *const () as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGPROF, &sa, core::ptr::null_mut());
            s as usize
        }
    });
    SAMPLE_PC.store(0, Ordering::Release);
    // SAFETY: signalling a live thread of ours; the wait has a deadline
    unsafe {
        if libc::pthread_kill(t.0, libc::SIGPROF) != 0 {
            return None;
        }
        let mut ts: libc::timespec = core::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
        ts.tv_nsec += 50_000_000;
        if ts.tv_nsec >= 1_000_000_000 {
            ts.tv_sec += 1;
            ts.tv_nsec -= 1_000_000_000;
        }
        while libc::sem_timedwait(sem as *mut libc::sem_t, &ts) != 0 {
            if *libc::__errno_location() != libc::EINTR {
                return None;
            }
        }
    }
    let pc = SAMPLE_PC.load(Ordering::Acquire);
    (pc != 0).then_some(pc)
}

/// A thread's CPU time (user + system), seconds.
pub fn thread_cpu_seconds(t: HostThread) -> Option<f64> {
    // SAFETY: plain queries on a live thread of ours
    unsafe {
        let mut id: libc::clockid_t = 0;
        if libc::pthread_getcpuclockid(t.0, &mut id) != 0 {
            return None;
        }
        let mut ts: libc::timespec = core::mem::zeroed();
        (libc::clock_gettime(id, &mut ts) == 0).then(|| ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9)
    }
}

/// The monotonic clock in ns.
pub fn now_ns() -> u64 {
    let mut ts: libc::timespec = unsafe { core::mem::zeroed() };
    // SAFETY: a plain query
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// The machine's physical memory in bytes.
pub fn total_memory() -> u64 {
    // SAFETY: sysconf has no preconditions
    let (pages, size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    (pages.max(0) as u64) * (size.max(0) as u64)
}

/// This process's resident and swapped-out bytes (what Windows counts as the private commit a job limits, near
/// enough: Linux has no commit charge per process).
pub fn process_memory() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
            << 10
    };
    (kb("VmRSS:"), kb("VmSwap:"))
}

/// The process memory cap: a cgroup v2 of this process's own with memory.max (where the user's cgroup tree is
/// delegated, as systemd user sessions do), else a watchdog thread that stops the game, with the reason in its log,
/// once its resident + swapped memory passes the cap. Returns how it is enforced.
pub fn memory_cap(cap: u64) -> Result<String, String> {
    match cgroup_cap(cap) {
        Ok(p) => return Ok(format!("cgroup {p} memory.max")),
        Err(e) => crate::rt::log(&format!(
            "process memory cap: no cgroup of its own ({e}); a watchdog enforces it"
        )),
    }
    std::thread::Builder::new()
        .name("memory cap".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let (rss, swap) = process_memory();
            if rss + swap > cap {
                crate::rt::log(&format!(
                    "[RED] process memory cap: {} MB in use passes the cap of {} MB: the game stops here instead of the machine (SWR_MEMORY_CAP_GB sets the cap, 0 = off)",
                    (rss + swap) >> 20,
                    cap >> 20
                ));
                crate::process::exit(70);
            }
        })
        .map(|_| "watchdog on the resident size".to_string())
        .map_err(|e| e.to_string())
}

fn cgroup_cap(cap: u64) -> Result<String, String> {
    let line = std::fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
    let rel = line
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .ok_or("not cgroup v2")?
        .trim()
        .to_string();
    let root = std::path::Path::new("/sys/fs/cgroup");
    let own = root.join(rel.trim_start_matches('/'));
    let parent = own.parent().ok_or("no parent cgroup")?;
    let ctl = std::fs::read_to_string(parent.join("cgroup.subtree_control"))
        .map_err(|e| e.to_string())?;
    if !ctl.split_whitespace().any(|c| c == "memory") {
        return Err(format!(
            "{} does not delegate the memory controller",
            parent.display()
        ));
    }
    let mine = parent.join(format!("nxrs-{}.scope", std::process::id()));
    std::fs::create_dir(&mine).map_err(|e| format!("{}: {e}", mine.display()))?;
    let fail = |e: String| {
        let _ = std::fs::remove_dir(&mine);
        e
    };
    std::fs::write(mine.join("memory.max"), cap.to_string())
        .map_err(|e| fail(format!("memory.max: {e}")))?;
    let _ = std::fs::write(mine.join("memory.swap.max"), "0");
    std::fs::write(mine.join("cgroup.procs"), std::process::id().to_string())
        .map_err(|e| fail(format!("cgroup.procs: {e}")))?;
    Ok(mine.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_committed_runs() {
        let a = reserve_anywhere(16 * PAGE, false).unwrap();
        assert!(commit(a + 2 * PAGE, 3 * PAGE));
        assert_eq!(committed_runs(a, 16 * PAGE), vec![(a + 2 * PAGE, 3 * PAGE)]);
        assert_eq!(committed_in(a, 16 * PAGE), 3 * PAGE);
        assert_eq!(query(a + 3 * PAGE), Some((true, 2, a, 16 * PAGE)));
        assert_eq!(query(a), Some((false, 0, a, 16 * PAGE)));
        assert!(readable(a + 2 * PAGE, 3 * PAGE));
        assert!(!readable(a, PAGE));
        assert!(protect(a + 2 * PAGE, PAGE, 0));
        assert!(!readable(a + 2 * PAGE, 8));
        assert_eq!(allocation_of(a + PAGE, a + 4 * PAGE), a);
        release(a);
        assert_eq!(query(a), None);
    }

    static HIT: AtomicU64 = AtomicU64::new(0);
    unsafe fn commit_on_touch(f: &mut Fault) -> bool {
        let base = HIT.load(Ordering::Acquire);
        if f.sig == libc::SIGSEGV && base != 0 && f.addr >= base && f.addr < base + PAGE {
            commit(base, PAGE);
            HIT.store(0, Ordering::Release);
            return true;
        }
        false
    }

    #[test]
    fn a_fault_hook_retries_the_access() {
        let a = reserve_anywhere(PAGE, false).unwrap();
        HIT.store(a, Ordering::Release);
        add_fault_hook(commit_on_touch);
        unsafe { std::ptr::write_volatile((a + 8) as *mut u64, 5) };
        assert_eq!(unsafe { std::ptr::read_volatile((a + 8) as *const u64) }, 5);
        assert_eq!(HIT.load(Ordering::Acquire), 0);
        release(a);
    }

    #[test]
    fn thread_samples_and_cpu_time() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = stop.clone();
        let h = std::thread::spawn(move || {
            tx.send(current_thread()).unwrap();
            let mut x = 0u64;
            while !s.load(Ordering::Relaxed) {
                x = x.wrapping_add(1);
                std::hint::black_box(x);
            }
        });
        let t = rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(thread_pc(t).is_some_and(|pc| pc != 0));
        assert!(thread_cpu_seconds(t).is_some_and(|s| s > 0.0));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    /// AArch64 fault kinds: the ESR record is found among the sigcontext records and decoded as the kernel sets it.
    #[test]
    fn esr_records_and_access_kinds() {
        let rec = |magic: u32, size: u32, payload: &[u8]| {
            let mut v = magic.to_ne_bytes().to_vec();
            v.extend(size.to_ne_bytes());
            v.extend(payload);
            v.resize(size as usize, 0);
            v
        };
        // an FPSIMD record (528 bytes), then ESR, then the terminator
        let esr = (0x24u64 << 26) | (1 << 6) | 0x07; // data abort, lower EL, write, translation fault level 3
        let mut area = rec(0x4650_8001, 528, &[]);
        area.extend(rec(0x4553_5201, 16, &esr.to_ne_bytes()));
        area.extend(rec(0, 8, &[]));
        area.resize(4096, 0);
        assert_eq!(find_esr(&area), Some(esr));
        assert_eq!(find_esr(&[0u8; 64]), None);
        assert_eq!(esr_access(esr), (true, false));
        assert_eq!(esr_access(esr & !(1 << 6)), (false, false), "a read");
        assert_eq!(
            esr_access(esr | (1 << 8)),
            (false, false),
            "cache maintenance is not a write"
        );
        assert_eq!(
            esr_access(0x20u64 << 26),
            (false, true),
            "instruction abort"
        );
        assert_eq!(
            esr_access(0x15u64 << 26),
            (false, false),
            "an SVC is no access"
        );
    }
}

//! Indirect-branch dispatch: guest address -> lifted function, O(1).
//!
//! One `u32` slot per 4-byte guest instruction over [lo, hi), the span of every module's function starts. A slot is 0
//! when no function starts there, else the host function's address minus `anchor` (just below the lowest lifted
//! function), so 4 bytes suffice while all lifted code lies within 4 GiB, which one PE image guarantees. The gaps
//! between code ranges are never written, so the OS never backs them. The table is built once and made read-only;
//! any number of threads read it without locks. Host functions (the bridge, `hostfn.rs`) live outside the modules
//! and are found by the slow path, as are setjmp-owner resumes (`sjlj.rs`) and code the build missed (`interp`).

use crate::asm::NXRT_DISPATCH;
use crate::cpu::{Cpu, GuestFn};
use std::sync::OnceLock;

/// A lifted module as the generated code describes it (`RtModule`): its place in the guest
/// layout and the generator's `<name>_fn_addrs` / `<name>_fn_ptrs` / `<name>_fn_count`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RtModule {
    pub name: *const core::ffi::c_char,
    pub base: u64,
    pub size: u64,
    pub addrs: *const u64,
    pub fns: *const GuestFn,
    pub count: u64,
}

/// A module after validation.
#[derive(Clone)]
pub struct Module {
    pub name: String,
    pub base: u64,
    pub size: u64,
    pub addrs: &'static [u64],
    pub fns: &'static [GuestFn],
}

unsafe impl Send for Module {}
unsafe impl Sync for Module {}

impl Module {
    /// # Safety
    /// The pointers of `m` must stay valid for the process (they point into the linked game code).
    pub unsafe fn from_raw(m: &RtModule) -> Module {
        let name = std::ffi::CStr::from_ptr(m.name).to_string_lossy().into_owned();
        let n = m.count as usize;
        let (addrs, fns) = if n == 0 {
            (&[][..], &[][..])
        } else {
            (std::slice::from_raw_parts(m.addrs, n), std::slice::from_raw_parts(m.fns, n))
        };
        Module { name, base: m.base, size: m.size, addrs, fns }
    }
}

pub struct Table {
    pub modules: Vec<Module>,
    pub lo: u64,
    pub hi: u64,
    pub slots: u64,
    pub functions: u64,
    slot: *const u32,
    anchor: u64,
    /// (host address, guest address) of every lifted function, sorted by host address: the fault handler's map.
    by_host: Vec<(u64, u64)>,
}

unsafe impl Send for Table {}
unsafe impl Sync for Table {}

static TABLE: OnceLock<Table> = OnceLock::new();

pub const TABLE_SPAN_LIMIT: u64 = 1 << 32;

impl Table {
    /// Builds the table (unsorted or misaligned starts, overlapping modules, host code over 4 GiB are
    /// errors) and seals it read-only.
    pub fn build(mut modules: Vec<Module>) -> Result<Table, String> {
        modules.sort_by_key(|m| m.base);
        for w in modules.windows(2) {
            if w[0].base + w[0].size > w[1].base {
                return Err(format!("dispatch: modules {} and {} overlap", w[0].name, w[1].name));
            }
        }
        let (mut lo, mut hi) = (u64::MAX, 0u64);
        let (mut host_lo, mut host_hi) = (u64::MAX, 0u64);
        for m in &modules {
            for (k, (&a, &f)) in m.addrs.iter().zip(m.fns).enumerate() {
                if a & 3 != 0 || a.wrapping_sub(m.base) >= m.size {
                    return Err(format!("dispatch: {}_fn_addrs[{k}] = {a:#x} is misaligned or outside the module", m.name));
                }
                if k > 0 && a <= m.addrs[k - 1] {
                    return Err(format!("dispatch: {}_fn_addrs is not strictly ascending at [{k}] = {a:#x}", m.name));
                }
                let f = f as usize as u64;
                host_lo = host_lo.min(f);
                host_hi = host_hi.max(f);
            }
            if let (Some(&first), Some(&last)) = (m.addrs.first(), m.addrs.last()) {
                lo = lo.min(first);
                hi = hi.max(last + 4);
            }
        }
        if hi <= lo {
            if modules.is_empty() || modules.iter().any(|m| !m.addrs.is_empty()) {
                return Err("dispatch: no functions in any module".into());
            }
            // a build without lifted code (`nxrs-build --interp-only`): every dispatch goes to the interpreter
            crate::interp::set_only(true);
            crate::rt::log("dispatch: no lifted functions in any module (an --interp-only build): every guest instruction is interpreted");
            return Ok(Table { modules, lo: 0, hi: 0, slots: 0, functions: 0, slot: core::ptr::null(), anchor: 0, by_host: vec![] });
        }
        if hi - lo > TABLE_SPAN_LIMIT {
            return Err(format!("dispatch: modules span {:#x} guest bytes, over the flat table's limit", hi - lo));
        }
        let anchor = (host_lo & !15) - 16;
        if host_hi - anchor > u32::MAX as u64 {
            return Err(format!("dispatch: lifted code spans {:#x} host bytes; slots hold 32-bit offsets", host_hi - anchor));
        }
        let slots = (hi - lo) >> 2;
        let bytes = (slots * 4 + 0xFFFF) & !0xFFFF;
        let slot = crate::mem::host_alloc(bytes).ok_or("dispatch: cannot allocate the table")? as *mut u32;
        let mut functions = 0;
        let mut by_host = vec![];
        for m in &modules {
            for (&a, &f) in m.addrs.iter().zip(m.fns) {
                // SAFETY: a lies in [lo, hi), so the index is below slots.
                unsafe { *slot.add(((a - lo) >> 2) as usize) = (f as usize as u64 - anchor) as u32 };
                by_host.push((f as usize as u64, a));
                functions += 1;
            }
        }
        by_host.sort_unstable();
        crate::mem::host_protect_readonly(slot as u64, bytes);
        Ok(Table { modules, lo, hi, slots, functions, slot, anchor, by_host })
    }

    pub fn lookup(&self, pc: u64) -> Option<GuestFn> {
        let i = pc.wrapping_sub(self.lo).rotate_right(2);
        if i < self.slots {
            // SAFETY: i < slots.
            let off = unsafe { *self.slot.add(i as usize) };
            if off != 0 {
                // SAFETY: the slot holds a lifted function's offset from anchor.
                return Some(unsafe { core::mem::transmute::<usize, GuestFn>((self.anchor + off as u64) as usize) });
            }
        }
        None
    }

    pub fn module_at(&self, pc: u64) -> Option<&Module> {
        self.modules.iter().find(|m| pc.wrapping_sub(m.base) < m.size)
    }

    /// The guest function whose lifted code starts at or before host address `rip` (the nearest lifted function
    /// below it), with the distance. Exact when `rip` is inside lifted code; `fault.rs` checks that with the
    /// function table of the PE image first.
    pub fn guest_of_host(&self, rip: u64) -> Option<(u64, u64)> {
        let k = self.by_host.partition_point(|&(h, _)| h <= rip);
        if k == 0 {
            return None;
        }
        let (h, g) = self.by_host[k - 1];
        Some((g, rip - h))
    }

    /// Guest address of the lifted function starting exactly at host address `f`.
    pub fn guest_of_fn(&self, f: u64) -> Option<u64> {
        let k = self.by_host.partition_point(|&(h, _)| h < f);
        self.by_host.get(k).filter(|e| e.0 == f).map(|e| e.1)
    }
}

/// Installs the table for `rt_dispatch`. Once, before any guest thread starts.
pub fn install(t: Table) -> Result<&'static Table, String> {
    TABLE.set(t).map_err(|_| "dispatch: installed twice".to_string())?;
    let t = TABLE.get().unwrap();
    // SAFETY: written once, before any thread reads it (thread creation publishes it).
    unsafe {
        let d = &mut *core::ptr::addr_of_mut!(NXRT_DISPATCH);
        d.lo = t.lo;
        // NXRS_INTERP_ONLY: the fast path never hits, every dispatch is resolved (and interpreted)
        d.slots = if crate::interp::only() { 0 } else { t.slots };
        d.slot = t.slot;
        d.anchor = t.anchor;
    }
    Ok(t)
}

pub fn table() -> Option<&'static Table> {
    TABLE.get()
}

/// "0x7000001234 (main+0x1234)" for an address inside a module, else the bare address.
pub fn guest_name(pc: u64) -> String {
    if let Some(m) = table().and_then(|t| t.module_at(pc)) {
        return format!("{pc:#x} ({}+{:#x})", m.name, pc - m.base);
    }
    if let Some(n) = crate::hostfn::name_of(pc) {
        return format!("{pc:#x} (host {n})");
    }
    format!("{pc:#x}")
}

/// The slow path of `rt_dispatch` (`nxrt_dispatch_slow` in asm.rs): what to jump to for `c->pc` when the flat table
/// has no function there: a host function (through `nxrt_host_entry`), a guest jump into a live setjmp owner or out
/// to a barrier (`nxrt_jump_pending`, with `c->sj` set to the target slot), or the fatal miss. It always returns, so
/// no jump starts from a Rust frame.
#[no_mangle]
pub unsafe extern "C" fn nxrt_dispatch_resolve(c: *mut Cpu) -> GuestFn {
    crate::boundary::guard("rt_dispatch", || {
        let pc = (*c).pc;
        if crate::hostfn::lookup(pc).is_some() {
            return crate::asm::nxrt_host_entry as GuestFn;
        }
        if let Some(a) = crate::hostfn::aliased(pc) {
            (*c).pc = a;
            return crate::asm::nxrt_host_entry as GuestFn;
        }
        if !crate::interp::only() {
            if let Some(f) = table().and_then(|t| t.lookup(pc)) {
                return f; // only a racing reader before install
            }
        }
        if let Some(s) = crate::sjlj::jump_target(c) {
            (*c).sj = s as *mut _;
            (*c).sync_mxcsr();
            return crate::asm::nxrt_jump_pending as GuestFn;
        }
        // code the build did not translate: the interpreter (recorded for the next build)
        if let Some(f) = crate::interp::take(pc) {
            return f;
        }
        let why = if table().is_none() {
            "before the dispatch table was installed"
        } else if pc & 3 != 0 {
            "misaligned address"
        } else if table().unwrap().module_at(pc).is_some() {
            "not a function start, and no live setjmp owner holds it"
        } else {
            "outside every module, no host function registered"
        };
        crate::rt::guest_fatal(c, &format!("rt_dispatch: no function at guest pc {}: {why}", guest_name(pc)));
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn f1(c: *mut Cpu) {
        (*c).x[0] = 1;
    }
    unsafe extern "C" fn f2(c: *mut Cpu) {
        (*c).x[0] = 2;
    }
    unsafe extern "C" fn f3(c: *mut Cpu) {
        (*c).x[0] = 3;
    }

    fn leak<T: Clone>(v: &[T]) -> &'static [T] {
        Box::leak(v.to_vec().into_boxed_slice())
    }

    fn module(name: &str, base: u64, addrs: &[u64], fns: &[GuestFn]) -> Module {
        Module { name: name.into(), base, size: 0x10000, addrs: leak(addrs), fns: leak(fns) }
    }

    #[test]
    fn lookup_and_names() {
        let t = Table::build(vec![
            module("sdk", 0x7000_0200_0000, &[0x7000_0200_0010], &[f3]),
            module("main", 0x7000_0000_0000, &[0x7000_0000_0000, 0x7000_0000_0040], &[f1, f2]),
        ])
        .unwrap();
        assert_eq!(t.functions, 3);
        assert_eq!(t.lo, 0x7000_0000_0000);
        assert_eq!(t.hi, 0x7000_0200_0014);
        let call = |pc| {
            let mut c = Cpu::new_boxed();
            unsafe { t.lookup(pc).unwrap()(&mut *c) };
            c.x[0]
        };
        assert_eq!(call(0x7000_0000_0000), 1);
        assert_eq!(call(0x7000_0000_0040), 2);
        assert_eq!(call(0x7000_0200_0010), 3);
        assert!(t.lookup(0x7000_0000_0004).is_none(), "no start there");
        assert!(t.lookup(0x7000_0000_0042).is_none(), "misaligned");
        assert!(t.lookup(0x6FFF_FFFF_FFFC).is_none(), "below");
        assert!(t.lookup(0x7000_0200_0014).is_none(), "above");
        assert_eq!(t.module_at(0x7000_0200_0020).unwrap().name, "sdk");
        assert_eq!(t.guest_of_fn((f2 as GuestFn) as usize as u64), Some(0x7000_0000_0040));
        let (g, d) = t.guest_of_host((f3 as GuestFn) as usize as u64 + 2).unwrap();
        assert!(g == 0x7000_0200_0010 || d > 2); // nearest lifted function below
    }

    #[test]
    fn rejects_bad_tables() {
        assert!(Table::build(vec![module("m", 0x7000_0000_0000, &[0x7000_0000_0040, 0x7000_0000_0000], &[f1, f2])]).is_err());
        assert!(Table::build(vec![module("m", 0x7000_0000_0000, &[0x7000_0000_0002], &[f1])]).is_err());
        assert!(Table::build(vec![module("m", 0x7000_0000_0000, &[0x7000_0010_0000], &[f1])]).is_err());
        assert!(Table::build(vec![
            module("a", 0x7000_0000_0000, &[0x7000_0000_0000], &[f1]),
            module("b", 0x7000_0000_8000, &[0x7000_0000_8000], &[f2]),
        ])
        .is_err());
        assert!(Table::build(vec![]).is_err());
    }
}

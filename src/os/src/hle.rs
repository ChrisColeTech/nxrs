//! Host implementations registered by the SDK's mangled names, on the runtime's bridge (`nxrt::hostfn`).
//!
//! * `force(name, f)`: every reference to `name` in every module (GOT, GLOB_DAT, ABS64; the SDK's calls between its
//!   own exports go through its PLT, so they are references too) binds to `f`, also when a module defines it. This
//!   is how the host replaces the SDK's service clients: their IPC never runs.
//! * `hle(name, f)`: only an import no module defines binds to `f` (C library and other runtime pieces the game
//!   expects the system to provide).
//! * `alias(name, target)`: every reference to `name` binds to the modules' own definition of `target`.
//! * Every import that is left unresolved after that binds to a logging stub that answers 0 and records an
//!   `UNIMPL Import` gap (instead of the runtime's fatal stub).
//!
//! The module images are read before the runtime loads them, so `hle` and the stubs know what the modules define.

use crate::guest::Args;
use nxrt::cpu::Cpu;
use nxrt::hostfn::{self, HostFn};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

pub type Result = u32;
pub const RES_OK: u64 = 0;
/// Generic failure for features the host cuts uniformly (networking and the like).
pub const RES_FAIL: u64 = 0x2A2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Force,
    Hle,
}

#[derive(Default)]
pub struct Registry {
    entries: Vec<(String, HostFn, Kind, &'static str)>,
    aliases: Vec<(String, String)>,
}

impl Registry {
    pub fn force(&mut self, name: &str, why: &'static str, f: HostFn) {
        self.entries.push((name.to_string(), f, Kind::Force, why));
    }
    pub fn hle(&mut self, name: &str, f: HostFn) {
        self.entries.push((name.to_string(), f, Kind::Hle, ""));
    }
    pub fn alias(&mut self, name: &str, target: &str) {
        self.aliases.push((name.to_string(), target.to_string()));
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.0.as_str())
    }
}

/// What the modules define and import, read from their images before loading.
#[derive(Default, Debug)]
pub struct ModuleSyms {
    /// (module, name, offset in the module) of every defined dynamic symbol.
    pub defined: Vec<(String, String, u64)>,
    /// Every symbol name some module references through a relocation or imports.
    pub referenced: HashSet<String>,
    /// Imports (undefined symbols) by module.
    pub imports: Vec<(String, String)>,
}

impl ModuleSyms {
    pub fn read(files: &[(String, Vec<u8>)]) -> std::result::Result<ModuleSyms, String> {
        let mut s = ModuleSyms::default();
        for (name, raw) in files {
            let img = nxrt::loader::parse(raw).map_err(|e| format!("{name}: {e}"))?;
            let d = nxrt::loader::dynamic(&img.bytes).map_err(|e| format!("{name}: {e}"))?;
            for sym in &d.syms {
                if sym.name.is_empty() {
                    continue;
                }
                if sym.defined() {
                    s.defined.push((name.clone(), sym.name.clone(), sym.value));
                } else {
                    s.imports.push((name.clone(), sym.name.clone()));
                }
            }
            for r in &d.relocs {
                if r.sym != 0 {
                    if let Some(sym) = d.syms.get(r.sym as usize) {
                        s.referenced.insert(sym.name.clone());
                    }
                }
            }
        }
        Ok(s)
    }

    pub fn defines(&self, name: &str) -> bool {
        self.defined.iter().any(|(_, n, _)| n == name)
    }
}

/// The installed registry's report.
#[derive(Debug, Default)]
pub struct Installed {
    pub forced: usize,
    pub hle: usize,
    pub aliased: usize,
    pub stubbed: Vec<String>,
}

/// Binds the registry and the stubs on the runtime's bridge (before `nxrt::loader::load`).
pub fn install(reg: &Registry, syms: &ModuleSyms) -> Installed {
    let mut out = Installed::default();
    let defined: HashSet<&str> = syms.defined.iter().map(|(_, n, _)| n.as_str()).collect();
    let mut provided: HashSet<String> = HashSet::new();
    for (name, f, kind, _why) in &reg.entries {
        match kind {
            Kind::Force => {
                hostfn::add_override(name, *f);
                provided.insert(name.clone());
                if syms.referenced.contains(name.as_str()) || syms.imports.iter().any(|(_, n)| n == name) {
                    out.forced += 1;
                }
            }
            Kind::Hle => {
                if !defined.contains(name.as_str()) {
                    hostfn::add_override(name, *f);
                    provided.insert(name.clone());
                    out.hle += 1;
                }
            }
        }
    }
    for (name, target) in &reg.aliases {
        if defined.contains(target.as_str()) {
            ALIASES.lock().unwrap().insert(name.clone(), target.clone());
            hostfn::add_override(name, alias_forward);
            provided.insert(name.clone());
            out.aliased += 1;
        } else {
            crate::log!("[RED] alias {name} -> {target}: no module defines the target; left as it binds");
        }
    }
    let mut stubbed: Vec<String> = syms
        .imports
        .iter()
        .map(|(_, n)| n.clone())
        .filter(|n| !defined.contains(n.as_str()) && !provided.contains(n) && !n.starts_with("__rel_"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    stubbed.dedup();
    for n in &stubbed {
        hostfn::add_override(n, unimpl_import);
    }
    out.stubbed = stubbed;
    out
}

static ALIASES: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// A reference to an alias: the call goes on to the target's own definition, with the caller's registers and
/// stack arguments as they are.
unsafe fn alias_forward(c: *mut Cpu) {
    let name = hostfn::name_of((*c).pc).unwrap_or_default();
    let target = ALIASES.lock().unwrap().get(&name).cloned().unwrap_or_default();
    let Some(t) = sym(&target) else {
        nxrt::rt::guest_fatal(c, &format!("alias {name}: target {target} is not loaded"));
    };
    let lr = (*c).x[30];
    if nxrt::boundary::call_guest(c, t) != nxrt::boundary::GuestCall::Returned {
        return;
    }
    (*c).x[30] = lr;
}

unsafe fn unimpl_import(c: *mut Cpu) {
    let name = hostfn::name_of((*c).pc).unwrap_or_default();
    let a = Args(&mut *c);
    crate::log::gap(crate::log::Gap::Import, &name, &format!("x0={:#x} x1={:#x} x2={:#x}, answered 0", a.x(0), a.x(1), a.x(2)));
    (*c).x[0] = 0;
}

// ---- the guest's symbols, once loaded

static SYMS: OnceLock<HashMap<String, u64>> = OnceLock::new();

/// Records the loaded modules' defined symbols (absolute addresses).
pub fn set_symbols(loaded: &[nxrt::loader::Loaded]) {
    let mut m = HashMap::new();
    for l in loaded {
        for s in &l.dynamic.syms {
            if s.defined() && !s.name.is_empty() {
                m.entry(s.name.clone()).or_insert(l.base + s.value);
            }
        }
    }
    let _ = SYMS.set(m);
}

/// The address of a symbol a loaded module defines.
pub fn sym(name: &str) -> Option<u64> {
    SYMS.get()?.get(name).copied()
}

/// A host function's guest address (stable per name and function), for vtables and proc tables the host hands out.
pub fn addr(name: &str, f: HostFn) -> u64 {
    hostfn::register(name, f)
}

/// Kernel object handles the os layer hands out itself sit outside the runtime kernel's table (which starts at
/// 0x1001): sessions from `SESSION_BASE`, transfer and shared memory from `MEMORY_BASE`.
pub const SESSION_BASE: u32 = 0x000D_0000;
pub const MEMORY_BASE: u32 = 0x00D1_0000;

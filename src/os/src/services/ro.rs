//! nn::ro (NRO modules loaded at run time) and the parts of nn::jit that need no plugin. On the console both go
//! through a service (ldr:ro, jit); the host has neither, and the SDK's own Initialize reaches for a session it
//! never gets, so the SDK's calls are taken over by name (nxnative's `hle_ro.c` design).
//!
//! **NRO modules.** The build found the NROs the game's NRRs register in its RomFS and translated them with the
//! ExeFS at fixed bases after it (`translator::package::nro`, `<data>\nro.tsv`): the module window covers them and the
//! dispatch table has their functions, but nothing is mapped at start. `LoadModule` finds the module by the image's
//! build id (the size checked), maps its place, copies the image the game read and zeroes its bss, relocates it
//! against the modules loaded then (the ExeFS modules, then the NROs in load order, as Horizon's rtld does), binds the
//! imports of the modules loaded before it that only it defines (Smash's `main` imports its fighters' `lua2cpp`
//! functions from them), and calls its DT_INIT on the caller's thread. The image and buffer the game passes stay
//! the game's (on Horizon they become the module's memory). `UnloadModule` calls DT_FINI and binds every import that
//! pointed into the module again. A module not built (no row) fails with ro's failure, as on the console for one the
//! NRRs do not register; an online SDK (a `refused` row) fails so the game plays offline.
//! RegisterModuleInfo checks the NRR's form only (the build collected the modules by its hashes).
//! GetJitEnvironmentInfo reads the environment's own layout (the SDK's GetImpl(handle) is the handle: code memory
//! address and size at +8 / +0x10, data memory at +0x18 / +0x20).

use crate::guest::{r32, r8, w64, Args};
use crate::hle::Registry;
use nxrt::cpu::Cpu;
use nxrt::loader::{Dynamic, Loaded};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// A failure under ro's module number (22); the games check IsSuccess only.
const RO_FAIL: u64 = 22 | (1 << 9);
/// The SDK's result for an image without the NRO0 magic.
const RO_NOT_NRO: u64 = 0x816;

const R_ABS64: u32 = 257;
const R_GLOB_DAT: u32 = 1025;
const R_JUMP_SLOT: u32 = 1026;
const R_RELATIVE: u32 = 1027;

struct Nro {
    name: String,
    base: u64,
    size: u64,
    nro_size: u64,
    build_id: [u8; 32],
    /// Its image and dynamic section once loaded the first time.
    module: Option<Loaded>,
    loaded: bool,
    loads: u32,
    init: u64,
    fini: u64,
}

#[derive(Default)]
struct State {
    /// The ExeFS modules, in load order.
    base: Vec<Loaded>,
    nros: Vec<Nro>,
    /// Indexes into `nros` of the loaded ones, in load order.
    order: Vec<usize>,
    /// Build ids of online SDKs the build left out.
    refused: Vec<[u8; 32]>,
}

static ST: OnceLock<Mutex<State>> = OnceLock::new();

fn st() -> std::sync::MutexGuard<'static, State> {
    ST.get_or_init(|| Mutex::new(State::default())).lock().unwrap_or_else(|p| p.into_inner())
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut b = [0u8; 32];
    for (i, o) in b.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(b)
}

fn num(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// The NRO modules of the build (`<data>\nro.tsv`), read at boot: the module names `boot` must not load from the
/// ExeFS.
pub fn read_table(data: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(data.join("nro.tsv")) else { return vec![] };
    let mut s = st();
    for l in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 6 {
            continue;
        }
        let Some(bid) = hex32(f[5]) else { continue };
        if f[0] == "refused" {
            s.refused.push(bid);
            continue;
        }
        let (Some(base), Some(size), Some(nro_size)) = (num(f[1]), num(f[2]), num(f[3])) else { continue };
        s.nros.push(Nro { name: f[0].to_string(), base, size, nro_size, build_id: bid, module: None, loaded: false, loads: 0, init: 0, fini: 0 });
    }
    if !s.nros.is_empty() {
        crate::log!("ro: {} NRO modules built for loading at run time ({} online SDKs refused)", s.nros.len(), s.refused.len());
    }
    s.nros.iter().map(|n| n.name.clone()).collect()
}

/// The ExeFS modules as loaded (after relocation), for the NROs' symbol lookups.
pub fn set_base(loaded: &[Loaded]) {
    st().base = loaded.to_vec();
}

fn defines(m: &Loaded, name: &str) -> Option<u64> {
    m.dynamic.syms.iter().find(|s| s.defined() && s.name == name).map(|s| m.base.wrapping_add(s.value))
}

/// A symbol as rtld binds it now: a host function by that name, else the first definition in the ExeFS modules,
/// then the loaded NROs in load order.
fn resolve(s: &State, name: &str) -> Option<u64> {
    let stub = crate::hle::stubbed_import(name);
    if !stub {
        if let Some(a) = nxrt::hostfn::bind(name) {
            return Some(a);
        }
    }
    let found = s.base.iter().find_map(|m| defines(m, name)).or_else(|| s.order.iter().find_map(|&i| s.nros[i].module.as_ref().and_then(|m| defines(m, name))));
    // an import only a module not loaded now defines keeps its reporting stub
    found.or_else(|| if stub { nxrt::hostfn::bind(name) } else { None })
}

/// The value a symbol relocation of `m` takes now (None: leave the slot, a weak symbol nothing defines is 0).
fn bind(s: &State, m: &Loaded, sym: u32, typ: u32, addend: i64) -> u64 {
    let Some(y) = m.dynamic.syms.get(sym as usize) else { return 0 };
    let t = if y.name.is_empty() {
        Some(m.base.wrapping_add(y.value))
    } else {
        resolve(s, &y.name)
    };
    let add = if typ == R_JUMP_SLOT { 0 } else { addend as u64 };
    match t {
        Some(t) => t.wrapping_add(add),
        None if y.weak() => 0,
        None => nxrt::hostfn::unresolved(&y.name).wrapping_add(add),
    }
}

fn dyn_tag(img: &[u8], d: &Dynamic, want: u64) -> u64 {
    let mut o = d.dynamic as usize;
    while let (Some(t), Some(v)) = (img.get(o..o + 8), img.get(o + 8..o + 16)) {
        let (t, v) = (u64::from_le_bytes(t.try_into().unwrap()), u64::from_le_bytes(v.try_into().unwrap()));
        if t == 0 {
            break;
        }
        if t == want {
            return v;
        }
        o += 16;
    }
    0
}

/// Relocations of the loaded modules (except `skip`) bound again: those naming a symbol they do not define that
/// `nro` defines (load), or whose slot points into `range` (unload). Returns how many changed.
fn rebind(s: &State, skip: usize, nro: &Loaded, load: bool) -> u32 {
    let (lo, hi) = (nro.base, nro.base + nro.size);
    let mut n = 0;
    let mods: Vec<&Loaded> = s.base.iter().chain(s.order.iter().filter(|&&i| i != skip).filter_map(|&i| s.nros[i].module.as_ref())).collect();
    for m in mods {
        for r in &m.dynamic.relocs {
            if !matches!(r.typ, R_ABS64 | R_GLOB_DAT | R_JUMP_SLOT) || r.sym == 0 {
                continue;
            }
            let Some(y) = m.dynamic.syms.get(r.sym as usize) else { continue };
            let at = m.base + r.off;
            let add = if r.typ == R_JUMP_SLOT { 0 } else { r.addend as u64 };
            let cur = crate::guest::r64(at).wrapping_sub(add);
            let hit = if load { y.shndx == 0 && !y.name.is_empty() && defines(nro, &y.name).is_some() } else { cur >= lo && cur < hi };
            if !hit {
                continue;
            }
            let v = bind(s, m, r.sym, r.typ, r.addend);
            if v != cur.wrapping_add(add) {
                w64(at, v);
                n += 1;
            }
        }
    }
    n
}

/// Maps, copies, relocates and binds the NRO `i` from the guest image `img`. Returns (init, imports of others
/// bound to it).
fn load(s: &mut State, i: usize, img: u64) -> Result<(u64, u32), String> {
    let space = crate::mem::get().space;
    let (base, size, nro_size, first) = (s.nros[i].base, s.nros[i].size, s.nros[i].nro_size, s.nros[i].module.is_none());
    let raw = crate::guest::bytes(img, nro_size);
    let image = nxrt::loader::parse(raw)?;
    if image.bytes.len() as u64 != size {
        return Err(format!("{}: the image is {:#x} bytes, the build laid it out for {size:#x}", s.nros[i].name, image.bytes.len()));
    }
    let dynamic = nxrt::loader::dynamic(&image.bytes)?;
    if first {
        space.map_module(base, size)?;
    } else {
        // a reload starts from the image again; the code pages are read-only since the first load
        nxrt::mem::protect(base, size, nxrt::mem::Prot::ReadWrite);
    }
    unsafe { core::ptr::copy_nonoverlapping(image.bytes.as_ptr(), base as *mut u8, size as usize) };
    let (init, fini) = (dyn_tag(&image.bytes, &dynamic, 12), dyn_tag(&image.bytes, &dynamic, 13));
    let text = image.text;
    let ro = (image.rodata.0, image.data.0 - image.rodata.0);
    let data_at = image.data.0;
    let m = Loaded { name: s.nros[i].name.clone(), base, size, image, dynamic };
    s.nros[i].module = Some(m);
    s.order.push(i);
    {
        let m = s.nros[i].module.as_ref().unwrap();
        for r in &m.dynamic.relocs {
            let at = base + r.off;
            if r.off + 8 > size {
                return Err(format!("{}: relocation at {:#x} outside the image", m.name, r.off));
            }
            let v = match r.typ {
                0 => continue,
                R_RELATIVE => base.wrapping_add(r.addend as u64),
                R_ABS64 | R_GLOB_DAT | R_JUMP_SLOT => bind(s, m, r.sym, r.typ, r.addend),
                t => return Err(format!("{}: unsupported relocation type {t} at {:#x}", m.name, r.off)),
            };
            w64(at, v);
        }
    }
    if first {
        space.protect_segment(base + text.0, text.1, nxrt::mem::ST_CODE, nxrt::mem::PERM_RX)?;
        space.protect_segment(base + ro.0, ro.1, nxrt::mem::ST_CODE, nxrt::mem::PERM_R)?;
        space.protect_segment(base + data_at, size - data_at, nxrt::mem::ST_CODEDATA, nxrt::mem::PERM_RW)?;
    } else {
        nxrt::mem::protect(base + text.0, data_at - text.0, nxrt::mem::Prot::Read);
    }
    let n = rebind(s, i, s.nros[i].module.as_ref().unwrap(), true);
    let e = &mut s.nros[i];
    e.loaded = true;
    e.loads += 1;
    e.init = init;
    e.fini = fini;
    Ok((init, n))
}

/// nn::ro::Module as the SDK fills it: +0 the module (here: its base), +8 state (1 = loaded), +0x10 image,
/// +0x18 buffer.
fn module_of(s: &State, rm: u64) -> Option<usize> {
    if rm == 0 || r32(rm + 8) != 1 {
        return None;
    }
    let base = crate::guest::r64(rm);
    s.nros.iter().position(|n| n.base == base && n.loaded)
}

/// LoadModule(Module*, const void *image, void *buffer, size_t size, int flag[, bool])
unsafe fn ro_load(c: *mut Cpu) {
    let (rm, img, buf, bufsize, flag) = ((*c).x[0], (*c).x[1], (*c).x[2], (*c).x[3], (*c).x[4]);
    let r = (|| -> u64 {
        if img == 0 || crate::guest::bytes(img + 0x10, 4) != b"NRO0" {
            return RO_NOT_NRO;
        }
        let bid: [u8; 32] = crate::guest::bytes(img + 0x40, 32).try_into().unwrap();
        let n = r32(img + 0x18) as u64;
        let hex: String = bid[..8].iter().map(|b| format!("{b:02x}")).collect();
        let (init, name, base, bound) = {
            let mut s = st();
            if s.refused.contains(&bid) {
                crate::log!("ro::LoadModule: refused {hex}… ({n} bytes): an online SDK (Epic Online Services), not loaded; the game plays offline");
                return RO_FAIL;
            }
            let Some(i) = s.nros.iter().position(|m| m.build_id == bid) else {
                crate::log!("[RED] ro::LoadModule: no NRO with build id {hex}… ({n} bytes) was built (not in nro.tsv): the load fails");
                return RO_FAIL;
            };
            if s.nros[i].loaded {
                crate::log!("[RED] ro::LoadModule: {} ({hex}…) is loaded already", s.nros[i].name);
                return RO_FAIL;
            }
            if s.nros[i].nro_size != n {
                crate::log!("[RED] ro::LoadModule: {} is {n:#x} bytes, nro.tsv says {:#x}: the load fails", s.nros[i].name, s.nros[i].nro_size);
                return RO_FAIL;
            }
            match load(&mut s, i, img) {
                Ok((init, bound)) => (init, s.nros[i].name.clone(), s.nros[i].base, bound),
                Err(e) => {
                    crate::log!("[RED] ro::LoadModule: {e}");
                    return RO_FAIL;
                }
            }
        };
        if rm != 0 {
            w64(rm, base);
            crate::guest::w32(rm + 8, 1);
            w64(rm + 0x10, img);
            w64(rm + 0x18, buf);
        }
        let loads = st().nros.iter().find(|m| m.base == base).map_or(0, |m| m.loads);
        crate::log!(
            "ro::LoadModule {name} at {base:#x} (build id {hex}…, {n:#x} bytes, buffer {bufsize:#x}, bind {}, load {loads}): {bound} imports of earlier modules bound to it; init {}",
            if flag & 1 != 0 { "now" } else { "lazy" },
            if init != 0 { "called" } else { "none" }
        );
        if init != 0 && crate::guest::call(c, base + init, &[]).is_none() {
            return u64::MAX;
        }
        0
    })();
    if r != u64::MAX {
        (*c).x[0] = r;
    }
}

unsafe fn ro_unload(c: *mut Cpu) {
    let rm = (*c).x[0];
    let (fini, base, name) = {
        let s = st();
        match module_of(&s, rm) {
            Some(i) => (s.nros[i].fini, s.nros[i].base, s.nros[i].name.clone()),
            None => {
                crate::log!("[RED] ro::UnloadModule: not a loaded module ({rm:#x})");
                return;
            }
        }
    };
    if fini != 0 && crate::guest::call(c, base + fini, &[]).is_none() {
        return;
    }
    let mut s = st();
    let Some(i) = s.nros.iter().position(|n| n.base == base) else { return };
    s.order.retain(|&k| k != i);
    s.nros[i].loaded = false;
    let m = s.nros[i].module.take().unwrap();
    let n = rebind(&s, i, &m, false);
    s.nros[i].module = Some(m);
    crate::guest::w32(rm + 8, 0);
    w64(rm, 0);
    crate::log!("ro::UnloadModule {name}: {n} imports that pointed into it bound again");
}

macro_rules! rfn {
    ($name:ident, |$c:ident, $a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name($c: *mut Cpu) {
            let mut $a = Args(&mut *$c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*$c).x[0] = r;
        }
    };
}

rfn!(ro_init, |c, a| {
    crate::log_once!("ro init", "ro::Initialize: no ldr:ro session (symbol lookups search the loaded modules)");
    0
});
rfn!(ro_lookup, |c, a| {
    let (out, name) = (a.x(0), a.x(1));
    let n = if name != 0 { crate::guest::cstr(name, 512) } else { String::new() };
    let found = crate::hle::sym(&n).or_else(|| {
        let s = st();
        s.order.iter().find_map(|&i| s.nros[i].module.as_ref().and_then(|m| defines(m, &n)))
    });
    match found {
        Some(p) => {
            crate::guest::out(out, p);
            0
        }
        None => {
            crate::log_once!("ro lookup", "[RED] ro::LookupSymbol cannot find {n} (first case)");
            RO_FAIL
        }
    }
});
// LookupModuleSymbol(uintptr_t *out, const Module *module, const char *name): a name the module does not define
// fails, as on Horizon (games probe for one).
rfn!(ro_lookup_module, |c, a| {
    let (out, rm, name) = (a.x(0), a.x(1), a.x(2));
    let n = if name != 0 { crate::guest::cstr(name, 512) } else { String::new() };
    let s = st();
    match module_of(&s, rm) {
        Some(i) => match s.nros[i].module.as_ref().and_then(|m| defines(m, &n)) {
            Some(p) => {
                crate::guest::out(out, p);
                0
            }
            None => RO_FAIL,
        },
        None => {
            crate::log_once!("ro lookup module", "[RED] ro::LookupModuleSymbol {n}: not a loaded module");
            RO_FAIL
        }
    }
});
// GetBufferSize(size_t *out, const void *image): the bss size from the NRO header, as the SDK computes it.
rfn!(ro_buffer_size, |c, a| {
    let (out, img) = (a.x(0), a.x(1));
    if img == 0 || crate::guest::bytes(img + 0x10, 4) != b"NRO0" {
        return RO_NOT_NRO;
    }
    crate::guest::out(out, r32(img + 0x38) as u64);
    0
});
// RegisterModuleInfo(info, const void *nrr): the NRR's form (version 1, the Switch 2's, has a header 0x180 bytes
// longer before the same fields: hash table offset and count).
rfn!(ro_register, |c, a| {
    let nrr = a.x(1);
    if nrr == 0 || crate::guest::bytes(nrr, 4) != b"NRR0" {
        return RO_FAIL;
    }
    let hdr = if r8(nrr + 5) == 1 { 0x4c0 } else { 0x340 };
    let count = r32(nrr + hdr + 4);
    crate::log!("ro::RegisterModuleInfo: NRR with {count} hashes");
    0
});
rfn!(ro_unregister, |c, a| { 0 });
rfn!(ro_finalize, |c, a| { 0 });

/// GetJitEnvironmentInfo(JitHandle): {code address, code size, data address, data size}, returned through x8.
unsafe fn jit_info(c: *mut Cpu) {
    let (out, h) = ((*c).x[8], (*c).x[0]);
    for k in 0..4u64 {
        w64(out + 8 * k, if h != 0 { crate::guest::r64(h + 8 + 8 * k) } else { 0 });
    }
}

pub fn register(r: &mut Registry) {
    for (n, w, f) in [
        ("_ZN2nn2ro10InitializeEv", "no ldr:ro: lookups search the loaded modules", ro_init as unsafe fn(*mut Cpu)),
        ("_ZN2nn2ro8FinalizeEv", "Initialize opened no ldr:ro session", ro_finalize),
        ("_ZN2nn2ro12LookupSymbolEPmPKc", "a definition in the loaded modules", ro_lookup),
        ("_ZN2nn2ro18LookupModuleSymbolEPmPKNS0_6ModuleEPKc", "a definition in one loaded NRO", ro_lookup_module),
        ("_ZN2nn2ro13GetBufferSizeEPmPKv", "the bss size from the NRO header", ro_buffer_size),
        ("_ZN2nn2ro18RegisterModuleInfoEPNS0_16RegistrationInfoEPKv", "an NRR: its form checked", ro_register),
        ("_ZN2nn2ro20UnregisterModuleInfoEPNS0_16RegistrationInfoE", "NRRs are not registered", ro_unregister),
        ("_ZN2nn2ro10LoadModuleEPNS0_6ModuleEPKvPvmi", "an NRO translated at build time: mapped at its place, relocated, its init called", ro_load),
        ("_ZN2nn2ro10LoadModuleEPNS0_6ModuleEPKvPvmib", "an NRO translated at build time (the form with the bool)", ro_load),
        ("_ZN2nn2ro12UnloadModuleEPNS0_6ModuleE", "its fini, then out of the symbol lookups", ro_unload),
        ("_ZN2nn3jit21GetJitEnvironmentInfoENS0_9JitHandleE", "the environment's memory, from its own layout", jit_info),
    ] {
        r.force(n, w, f);
    }
}

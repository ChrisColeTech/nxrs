//! nn::ro (NRO modules loaded at run time) and the parts of nn::jit that need no plugin. On the console both go
//! through a service (ldr:ro, jit); the host has neither, and the SDK's own Initialize reaches for a session it
//! never gets. Symbol lookups search the loaded modules directly; the host loads no NRO at run time (no module
//! rewritten offline is mapped for it), so LoadModule fails with ro's failure the way it does on the console for a
//! module the NRRs do not register, and LookupModuleSymbol knows no module. RegisterModuleInfo checks the NRR's
//! form only (the console checks a module's hash against it at load, which never happens here).
//! GetJitEnvironmentInfo reads the environment's own layout (the SDK's GetImpl(handle) is the handle: code memory
//! address and size at +8 / +0x10, data memory at +0x18 / +0x20).

use crate::guest::{r32, r8, w64, Args};
use crate::hle::Registry;
use nxrt::cpu::Cpu;

/// A failure under ro's module number (22); the games check IsSuccess only.
const RO_FAIL: u64 = 22 | (1 << 9);
/// The SDK's result for an image without the NRO0 magic.
const RO_NOT_NRO: u64 = 0x816;

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
    match crate::hle::sym(&n) {
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
rfn!(ro_lookup_module, |c, a| {
    let n = if a.x(2) != 0 { crate::guest::cstr(a.x(2), 512) } else { String::new() };
    crate::log_once!("ro lookup module", "[RED] ro::LookupModuleSymbol {n}: not a loaded module (the host loads no NRO at run time)");
    RO_FAIL
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
rfn!(ro_load, |c, a| {
    let img = a.x(1);
    if img == 0 || crate::guest::bytes(img + 0x10, 4) != b"NRO0" {
        return RO_NOT_NRO;
    }
    crate::log!("[RED] ro::LoadModule: an NRO of {:#x} bytes: the host loads no NRO at run time; the load fails", r32(img + 0x18));
    RO_FAIL
});
rfn!(ro_unload, |c, a| {
    crate::log!("[RED] ro::UnloadModule: not a loaded module ({:#x})", a.x(0));
    0
});

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
        ("_ZN2nn2ro12LookupSymbolEPmPKc", "a definition in the loaded modules", ro_lookup),
        ("_ZN2nn2ro18LookupModuleSymbolEPmPKNS0_6ModuleEPKc", "no NRO is loaded at run time", ro_lookup_module),
        ("_ZN2nn2ro13GetBufferSizeEPmPKv", "the bss size from the NRO header", ro_buffer_size),
        ("_ZN2nn2ro18RegisterModuleInfoEPNS0_16RegistrationInfoEPKv", "an NRR: its form checked", ro_register),
        ("_ZN2nn2ro10LoadModuleEPNS0_6ModuleEPKvPvmi", "the host loads no NRO at run time: fails", ro_load),
        ("_ZN2nn2ro10LoadModuleEPNS0_6ModuleEPKvPvmib", "the host loads no NRO at run time: fails", ro_load),
        ("_ZN2nn2ro12UnloadModuleEPNS0_6ModuleE", "no NRO is loaded at run time", ro_unload),
        ("_ZN2nn3jit21GetJitEnvironmentInfoENS0_9JitHandleE", "the environment's memory, from its own layout", jit_info),
    ] {
        r.force(n, w, f);
    }
}

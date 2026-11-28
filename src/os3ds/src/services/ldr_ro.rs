//! ldr:ro: the CRO loader (3dbrew `RO Services`). The piece this wave owns is the module-base
//! write (`place_module`: runtime3ds's `Library::place` stores the load address in the generated
//! module's `base` variable so its code runs wherever the loader put it). Mapping a CRO,
//! applying its relocations and running its init are not built yet, so every RO command is
//! refused with a gap line rather than answered as if a module had loaded. Sonic Generations
//! ships no CRO (its code table has 0 modules).

use crate::hle::{Host, Reply, Request, R_INVALID_CMD};
use runtime3ds::dispatch::Library;
use runtime3ds::Process3ds;

pub const INITIALIZE: u32 = 0x0001;
pub const LOAD_CRR: u32 = 0x0002;
pub const UNLOAD_CRR: u32 = 0x0003;
pub const LOAD_CRO: u32 = 0x0004;
pub const UNLOAD_CRO: u32 = 0x0005;
pub const LINK_CRO: u32 = 0x0006;
pub const UNLINK_CRO: u32 = 0x0007;
pub const SHUTDOWN: u32 = 0x0008;
pub const LOAD_CRO_NEW: u32 = 0x0009;

/// writes a module's load address where the generated code reads it; says whether the module exists
pub fn place_module(library: &mut Library, name: &str, base: u32) -> bool {
    match library.module_index(name) {
        Some(index) => {
            library.place(index, base);
            true
        }
        None => false,
    }
}

pub fn command(host: &Host, _process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    let what = match request.cmd {
        INITIALIZE => "Initialize",
        LOAD_CRR => "LoadCRR",
        UNLOAD_CRR => "UnloadCRR",
        LOAD_CRO | LOAD_CRO_NEW => "LoadCRO",
        UNLOAD_CRO => "UnloadCRO",
        LINK_CRO => "LinkCRO",
        UNLINK_CRO => "UnlinkCRO",
        SHUTDOWN => "Shutdown",
        _ => "an unknown command",
    };
    host.gap(format!(
        "ldr:ro: {what} ({:#06x}) refused: CRO mapping/relocation is not built",
        request.cmd
    ));
    Reply::err(R_INVALID_CMD)
}

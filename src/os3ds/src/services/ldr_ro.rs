//! ldr:ro: the CRO loader. This wave implements the module-base write (runtime3ds's `Library::place`
//! writes the load address into the generated module's `base` variable) and answers the load/unload
//! handshake. No CRO is present in this title, so a load gaps visibly rather than pretending.

use crate::hle::{Host, Reply, Request, R_INVALID_CMD};
use runtime3ds::dispatch::Library;
use runtime3ds::Process3ds;

pub const INITIALIZE: u32 = 0x0001;
pub const LOAD_CRO: u32 = 0x0002;
pub const UNLOAD_CRO: u32 = 0x0003;
pub const LINK_CRO: u32 = 0x0004;
pub const UNLINK_CRO: u32 = 0x0005;
pub const SHUTDOWN: u32 = 0x0006;
pub const LOAD_CRO_NEW: u32 = 0x0007;

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

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        INITIALIZE | SHUTDOWN => Reply::ok(),
        LOAD_CRO | LOAD_CRO_NEW => {
            // the module name/path is in the request's path; without a CRO there is nothing to map
            host.gap("ldr:ro: LoadCRO requested, no CRO staged for this title (3DS-W3)");
            Reply::ok()
        }
        UNLOAD_CRO | LINK_CRO | UNLINK_CRO => {
            let _ = process;
            Reply::ok()
        }
        other => {
            host.gap(format!("ldr:ro: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle::Host;
    use runtime3ds::context::{self, Entry, Module, NO_ORIGIN};
    use runtime3ds::dispatch::Library;
    use std::ffi::c_char;

    #[repr(C)]
    struct RawModule {
        name: *const c_char,
        base: *mut u32,
        size: u32,
        count: u32,
        entries: *const Entry,
    }

    #[test]
    fn a_module_base_is_written_where_the_code_reads_it() {
        let name = b"Battle.cro\0";
        let mut base_var: u32 = 0;
        let raw = RawModule {
            name: name.as_ptr() as *const c_char,
            base: &mut base_var,
            size: 0x1000,
            count: 0,
            entries: std::ptr::null(),
        };
        let mut library = unsafe {
            Library::linked(
                context::ABI,
                [].as_ptr(),
                0,
                &raw as *const RawModule as *const Module,
                1,
                std::ptr::null(),
                0,
            )
        };
        assert_eq!(library.module_index("Battle.cro"), Some(0));
        assert!(place_module(&mut library, "Battle.cro", 0x0012_3400));
        assert_eq!(base_var, 0x0012_3400);
        assert!(!place_module(&mut library, "Missing.cro", 0));
        let _ = NO_ORIGIN;
        let _ = Host::new(std::env::temp_dir(), Vec::new());
    }
}

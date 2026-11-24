//! The service layer initialization `nninitInitializeSdkModule` calls: on the console each connects to a system
//! service; the host has none to connect to, so each returns at once and the service's later calls are taken over
//! one by one where they come into use.

use crate::hle::Registry;
use nxrt::cpu::Cpu;

unsafe fn noop(c: *mut Cpu) {
    let name = nxrt::hostfn::name_of((*c).pc).unwrap_or_default();
    crate::log_once!(format!("init {name}"), "init takeover: {name} (no-op)");
}

pub fn register(r: &mut Registry) {
    r.force(
        "nndiagStartup",
        "connect to the lm log service; the host prints diagnostic logs itself",
        noop,
    );
    r.force(
        "_ZN2nn2oe10InitializeEv",
        "connect to the am/oe environment services; environment queries are answered by the host",
        noop,
    );
    r.force(
        "_ZN2nn3aoc10InitializeEv",
        "add-on content service: the owned list comes from the host",
        noop,
    );
    r.force(
        "_ZN2nn3err26InitializeContextWriterApiEv",
        "the err error context service; the host does not report",
        noop,
    );
    r.force(
        "_ZN2nn2fs24InitializeForApplicationEv",
        "connect to fsp-srv; the file interface goes to the host file system and the RomFS",
        noop,
    );
    r.force(
        "_ZN2nn4pctl10InitializeEv",
        "parental controls; single-player has none",
        noop,
    );
    r.force(
        "_ZN2nn4diag34InitializeApplicationAbortObserverEv",
        "the system's abort observer; host crashes are reported by the runtime",
        noop,
    );
}

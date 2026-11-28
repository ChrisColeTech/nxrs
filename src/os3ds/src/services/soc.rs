//! soc: the socket service (3dbrew `Socket Services`). Initialize and Shutdown answer (the
//! shared work buffer the app hands over needs no host action); every socket call is refused
//! with a gap line (plan §4: sockets are deferred, the wasm lane refuses per title too).

use crate::hle::{Host, Reply, Request, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const INITIALIZE_SOCKETS: u32 = 0x0001;
pub const SHUTDOWN_SOCKETS: u32 = 0x0019;

pub fn command(host: &Host, _process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        INITIALIZE_SOCKETS | SHUTDOWN_SOCKETS => Reply::ok(),
        other => {
            host.gap(format!("soc: command {other:#06x} refused (sockets are deferred)"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

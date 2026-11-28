//! y2r: the YUV->RGB conversion service (3dbrew `Y2R Services`). The driver open/close answers;
//! every conversion command is refused with a gap line (no title has needed it yet; plan §4 puts
//! the conversion on the CPU or a compute pass when one does).

use crate::hle::{Host, Reply, Request, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const DRIVER_INITIALIZE: u32 = 0x002B;
pub const DRIVER_FINALIZE: u32 = 0x002C;

pub fn command(host: &Host, _process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        DRIVER_INITIALIZE | DRIVER_FINALIZE => Reply::ok(),
        other => {
            host.gap(format!("y2r: command {other:#06x} refused (no YUV conversion yet)"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

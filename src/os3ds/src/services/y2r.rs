//! y2r: the YUV->RGB conversion service. The handshake and the parameter setters answer; the
//! conversion itself is gap-refused (no title needs it to reach its menu) and the ledger says so.

use crate::hle::{Host, Reply, Request, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const INITIALIZE: u32 = 0x0001;
pub const SET_INPUT_FORMAT: u32 = 0x0002;
pub const SET_OUTPUT_FORMAT: u32 = 0x0003;
pub const SET_COEFFICIENTS: u32 = 0x0005;
pub const START_CONVERSION: u32 = 0x001A;

pub fn command(host: &Host, _process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        INITIALIZE | SET_INPUT_FORMAT | SET_OUTPUT_FORMAT | SET_COEFFICIENTS => Reply::ok(),
        START_CONVERSION => {
            host.gap("y2r: StartConversion not executed (no title game needs it yet)");
            Reply::ok()
        }
        other => {
            host.gap(format!("y2r: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

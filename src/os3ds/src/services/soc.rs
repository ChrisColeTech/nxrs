//! soc: the socket service. The buffer handshake and the state queries answer; connecting is
//! gap-refused (the wasm lane refuses per title too, plan §4's table).

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const INITIALIZE: u32 = 0x0001;
pub const SHUTDOWN: u32 = 0x0002;
pub const GET_SOCKET: u32 = 0x0003;
pub const CONNECT: u32 = 0x0004;
pub const SEND_TO: u32 = 0x0005;
pub const RECV_FROM: u32 = 0x0006;
pub const CLOSE: u32 = 0x0007;
pub const GETHOSTBYNAME: u32 = 0x0008;

pub fn command(host: &Host, _process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        INITIALIZE | SHUTDOWN | CLOSE => Reply::ok(),
        CONNECT | SEND_TO | RECV_FROM | GETHOSTBYNAME => {
            host.gap(format!("soc: command {:#x} not executed (sockets are deferred)", request.cmd));
            Reply::err(crate::hle::R_INVALID)
        }
        GET_SOCKET => Reply::new(vec![R_OK, 0, 0], 0),
        other => {
            host.gap(format!("soc: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

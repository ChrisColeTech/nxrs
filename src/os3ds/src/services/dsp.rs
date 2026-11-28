//! dsp::DSP: the handshake. Component loads and audio programs are answered with the state change
//! they expect but their payload is gap-refused in the ledger; W5 owns the DSP interpreter and the
//! media sink. No audio is produced, and the ledger says so.

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const LOAD_COMPONENT: u32 = 0x0001;
pub const UNLOAD_COMPONENT: u32 = 0x0002;
pub const GET_SEMAPHORE: u32 = 0x0003;
pub const GET_SEMAPHORE2: u32 = 0x0004;

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        LOAD_COMPONENT => {
            host.gap("dsp: LoadComponent accepted, firmware/audio payload not executed (3DS-W5)");
            Reply::ok()
        }
        UNLOAD_COMPONENT => Reply::ok(),
        GET_SEMAPHORE | GET_SEMAPHORE2 => {
            let event = process.create_event(1);
            Reply::new(vec![R_OK, 0, event], 2)
        }
        other => {
            host.gap(format!("dsp: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

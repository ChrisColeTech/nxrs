//! gsp::Gpu: the handshake and the shared memory. `RegisterInterruptRelayQueue` (3dbrew rev 24155)
//! returns a real shared block and the client index; the command queue (`GSP Shared Memory` rev
//! 24158) is drained but its GX payload is gap-refused in the ledger, never executed and never a
//! silent success. W4 owns the PICA path.

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID_CMD, SharedKind, CLIENT_ID};
use runtime3ds::Process3ds;

pub const REGISTER_INTERRUPT_RELAY_QUEUE: u32 = 0x0013;
pub const TRIGGER_CMD_REQ_QUEUE: u32 = 0x000C;
pub const SHARED_SIZE: u32 = 0x1000;
/// the command queue's offset for this client (shared base + 0x800 + client * 0x200)
pub const COMMAND_QUEUE: u32 = 0x800 + CLIENT_ID * 0x200;

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        REGISTER_INTERRUPT_RELAY_QUEUE => {
            // word 1 = flags (normal); translate = descriptor 0 + KEvent handle
            let event = request.translated.get(1).copied().unwrap_or(0);
            if event != 0 {
                host.gsp_event.store(event, std::sync::atomic::Ordering::Release);
            }
            let Some(shared) = host.register_shared(process, SharedKind::Gsp, SHARED_SIZE) else {
                return Reply::err(crate::hle::R_INVALID);
            };
            host.say(format!(
                "gsp: RegisterInterruptRelayQueue flags {:#x}, event {event:#x} -> shared {shared:#x}, client {CLIENT_ID}",
                request.word(0)
            ));
            // response: result, GSP thread index, descriptor 0 + shared memory handle
            Reply::new(vec![R_OK, CLIENT_ID, 0, shared], 2)
        }
        TRIGGER_CMD_REQ_QUEUE => {
            let address = host.gsp_address.load(std::sync::atomic::Ordering::Acquire);
            if address != 0 {
                let queue = address + COMMAND_QUEUE;
                let total = process.memory.read8(queue + 1);
                if total > 0 {
                    host.gap(format!(
                        "gsp: TriggerCmdReqQueue: {total} GX command(s) not executed (PICA is 3DS-W4)"
                    ));
                    // mark the queue drained so the guest's next trigger starts clean
                    process.memory.write8(queue, total);
                    process.memory.write8(queue + 1, 0);
                }
            }
            Reply::ok()
        }
        other => {
            host.gap(format!("gsp: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

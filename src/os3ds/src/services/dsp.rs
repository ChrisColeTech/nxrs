//! dsp::DSP: the handshake (3dbrew `DSP Services` command table). Address conversion, cache
//! maintenance (free on the host), the semaphore event and the headphone query are answered for
//! real. The firmware load is accepted and named in the gap ledger; the pipe traffic and the
//! semaphore writes the audio firmware would answer are refused with gap lines. W5 owns the DSP
//! program interpreter and the media sink; no audio is produced, and the ledger says so.

use crate::hle::{Host, Reply, Request, R_INVALID_CMD, R_OK};
use runtime3ds::Process3ds;

pub const RECV_DATA: u32 = 0x0001;
pub const RECV_DATA_IS_READY: u32 = 0x0002;
pub const SEND_DATA: u32 = 0x0003;
pub const SEND_DATA_IS_EMPTY: u32 = 0x0004;
pub const SET_SEMAPHORE: u32 = 0x0007;
pub const CONVERT_PROCESS_ADDRESS_FROM_DSP_DRAM: u32 = 0x000C;
pub const WRITE_PROCESS_PIPE: u32 = 0x000D;
pub const READ_PIPE_IF_POSSIBLE: u32 = 0x0010;
pub const LOAD_COMPONENT: u32 = 0x0011;
pub const UNLOAD_COMPONENT: u32 = 0x0012;
pub const FLUSH_DATA_CACHE: u32 = 0x0013;
pub const INVALIDATE_DATA_CACHE: u32 = 0x0014;
pub const REGISTER_INTERRUPT_EVENTS: u32 = 0x0015;
pub const GET_SEMAPHORE_EVENT_HANDLE: u32 = 0x0016;
pub const SET_SEMAPHORE_MASK: u32 = 0x0017;
pub const GET_HEADPHONE_STATUS: u32 = 0x001F;

/// the DSP's DRAM as the app sees it (3dbrew DSP:ConvertProcessAddressFromDspDram)
pub fn dram_address(dsp_address: u32) -> u32 {
    (dsp_address << 1).wrapping_add(0x1FF4_0000)
}

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        LOAD_COMPONENT => {
            // 3dbrew DSP:LoadComponent (0x001100C2): size, program mask, data mask; translate: the
            // component buffer. Response: result, u8 loaded, the buffer descriptor + pointer.
            host.gap(format!(
                "dsp: LoadComponent ({:#x} bytes) accepted; the DSP firmware does not run (3DS-W5)",
                request.word(0)
            ));
            let descriptor = request.translated.first().copied().unwrap_or(0);
            let buffer = request.translated.get(1).copied().unwrap_or(0);
            Reply::new(vec![R_OK, 1, descriptor, buffer], 2)
        }
        UNLOAD_COMPONENT => Reply::ok(),
        // the host's view of guest memory is always coherent
        FLUSH_DATA_CACHE | INVALIDATE_DATA_CACHE => Reply::ok(),
        CONVERT_PROCESS_ADDRESS_FROM_DSP_DRAM => {
            Reply::new(vec![R_OK, dram_address(request.word(0))], 0)
        }
        GET_SEMAPHORE_EVENT_HANDLE => {
            let event = process.create_event(0);
            host.say(format!("dsp: GetSemaphoreEventHandle -> {event:#x}"));
            Reply::new(vec![R_OK, 0, event], 2)
        }
        REGISTER_INTERRUPT_EVENTS => {
            host.say(format!(
                "dsp: RegisterInterruptEvents interrupt {} channel {} event {:#x}",
                request.word(0),
                request.word(1),
                request.translated.get(1).copied().unwrap_or(0)
            ));
            Reply::ok()
        }
        SET_SEMAPHORE_MASK => Reply::ok(),
        // 0 = no headphones
        GET_HEADPHONE_STATUS => Reply::new(vec![R_OK, 0], 0),
        RECV_DATA | RECV_DATA_IS_READY | SEND_DATA | SEND_DATA_IS_EMPTY | SET_SEMAPHORE
        | WRITE_PROCESS_PIPE | READ_PIPE_IF_POSSIBLE => {
            host.gap(format!(
                "dsp: command {:#06x} needs the running DSP firmware (3DS-W5)",
                request.cmd
            ));
            Reply::err(R_INVALID_CMD)
        }
        other => {
            host.gap(format!("dsp: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

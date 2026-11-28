//! gsp::Gpu: the handshake, the shared memory and the GX command queue (3dbrew `GSPGPU`,
//! `GSP Shared Memory`). `RegisterInterruptRelayQueue` returns a real shared block and the
//! client index; the frame clock pushes VBlank0/VBlank1 into the client's interrupt queue.
//! GX commands the CPU can do for real are done for real (DMA request: a copy, then the DMA
//! interrupt; memory fill: the fill, then PSC0/PSC1; flush cache regions: nothing to do on a
//! coherent host). The GPU commands (process command list, display transfer, texture copy) are
//! refused in the gap ledger and raise no completion interrupt: W4 owns the PICA path.

use crate::hle::{push_interrupt, Host, Reply, Request, SharedKind, CLIENT_ID, R_INVALID_CMD, R_OK};
use runtime3ds::Process3ds;

pub const WRITE_HW_REGS: u32 = 0x0001;
pub const WRITE_HW_REGS_WITH_MASK: u32 = 0x0002;
pub const WRITE_HW_REG_REPEAT: u32 = 0x0003;
pub const READ_HW_REGS: u32 = 0x0004;
pub const SET_BUFFER_SWAP: u32 = 0x0005;
pub const FLUSH_DATA_CACHE: u32 = 0x0008;
pub const INVALIDATE_DATA_CACHE: u32 = 0x0009;
pub const SET_LCD_FORCE_BLACK: u32 = 0x000B;
pub const TRIGGER_CMD_REQ_QUEUE: u32 = 0x000C;
pub const SET_AXI_CONFIG_QOS_MODE: u32 = 0x0010;
pub const REGISTER_INTERRUPT_RELAY_QUEUE: u32 = 0x0013;
pub const UNREGISTER_INTERRUPT_RELAY_QUEUE: u32 = 0x0014;
pub const ACQUIRE_RIGHT: u32 = 0x0016;
pub const RELEASE_RIGHT: u32 = 0x0017;
pub const IMPORT_DISPLAY_CAPTURE_INFO: u32 = 0x0018;
pub const SAVE_VRAM_SYS_AREA: u32 = 0x0019;
pub const RESTORE_VRAM_SYS_AREA: u32 = 0x001A;
pub const SET_INTERNAL_PRIORITIES: u32 = 0x001E;
pub const STORE_DATA_CACHE: u32 = 0x001F;

pub const SHARED_SIZE: u32 = 0x1000;
/// the command queue's offset for this client (shared base + 0x800 + client * 0x200)
pub const COMMAND_QUEUE: u32 = 0x800 + CLIENT_ID * 0x200;
/// the queue holds 15 commands of 0x20 bytes from +0x20
pub const QUEUE_COMMANDS: u32 = 15;
/// 3dbrew: RegisterInterruptRelayQueue's result for the first client (success level)
pub const R_FIRST_INITIALIZATION: u32 = 0x2A07;

/// GSP interrupt ids
pub const IRQ_PSC0: u8 = 0;
pub const IRQ_PSC1: u8 = 1;
pub const IRQ_DMA: u8 = 6;

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        REGISTER_INTERRUPT_RELAY_QUEUE => {
            // word 1 = flags; translate = copy-handle descriptor + the client's KEvent
            let event = request.translated.get(1).copied().unwrap_or(0);
            let first = host.gsp_event.swap(event, std::sync::atomic::Ordering::AcqRel) == 0;
            let existing = host.state.lock().unwrap().gsp_shared;
            let shared = match existing {
                Some(shared) => shared,
                None => match host.register_shared(process, SharedKind::Gsp, SHARED_SIZE) {
                    Some(shared) => shared,
                    None => return Reply::err(crate::hle::R_INVALID),
                },
            };
            host.say(format!(
                "gsp: RegisterInterruptRelayQueue flags {:#x}, event {event:#x} -> shared {shared:#x}, client {CLIENT_ID}",
                request.word(0)
            ));
            // response: result, GSP thread index, copy-handle descriptor + shared block
            let result = if first { R_FIRST_INITIALIZATION } else { R_OK };
            Reply::new(vec![result, CLIENT_ID, 0, shared], 2)
        }
        UNREGISTER_INTERRUPT_RELAY_QUEUE => {
            host.gsp_event.store(0, std::sync::atomic::Ordering::Release);
            Reply::ok()
        }
        TRIGGER_CMD_REQ_QUEUE => {
            let address = host.gsp_address.load(std::sync::atomic::Ordering::Acquire);
            if address != 0 {
                run_queue(host, process, address);
            }
            Reply::ok()
        }
        // one client: the right is always granted; the bus priority and the cache need no host
        // action (the host's view of guest memory is coherent)
        ACQUIRE_RIGHT | RELEASE_RIGHT | SET_AXI_CONFIG_QOS_MODE | SET_INTERNAL_PRIORITIES
        | FLUSH_DATA_CACHE | INVALIDATE_DATA_CACHE | STORE_DATA_CACHE => Reply::ok(),
        SET_LCD_FORCE_BLACK => {
            host.say(format!("gsp: SetLcdForceBlack {}", request.word(0) & 1));
            Reply::ok()
        }
        // no system applet shares VRAM headless: nothing to save or restore
        SAVE_VRAM_SYS_AREA | RESTORE_VRAM_SYS_AREA => Reply::ok(),
        SET_BUFFER_SWAP => {
            host.gap("gsp: SetBufferSwap recorded, nothing is displayed (3DS-W4)");
            Reply::ok()
        }
        WRITE_HW_REGS | WRITE_HW_REGS_WITH_MASK | WRITE_HW_REG_REPEAT | READ_HW_REGS
        | IMPORT_DISPLAY_CAPTURE_INFO => {
            host.gap(format!(
                "gsp: command {:#06x} (GPU registers/display) refused (3DS-W4); register {:#x}",
                request.cmd,
                request.word(0)
            ));
            Reply::err(R_INVALID_CMD)
        }
        other => {
            host.gap(format!("gsp: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

/// drains the client's GX command queue (header byte 0 = index of the next command, byte 1 =
/// pending count; commands are 0x20 bytes from +0x20, word 0's low byte the command id)
pub fn run_queue(host: &Host, process: &Process3ds, shared: u32) {
    let memory = &process.memory;
    let queue = shared + COMMAND_QUEUE;
    let mut index = memory.read8(queue) as u32 % QUEUE_COMMANDS;
    let total = memory.read8(queue + 1) as u32;
    let interrupts = shared + CLIENT_ID * 0x40;
    for _ in 0..total {
        let at = queue + 0x20 + index * 0x20;
        let word = |i: u32| memory.read32(at + 4 * i);
        match word(0) & 0xFF {
            // DMA request: source, destination, size
            0 => {
                let (source, destination, size) = (word(1), word(2), word(3).min(0x0100_0000));
                let mut bytes = vec![0u8; size as usize];
                memory.read_bytes(source, &mut bytes);
                memory.write_bytes(destination, &bytes);
                host.say(format!("gsp: GX DMA {source:#x} -> {destination:#x} ({size:#x} bytes)"));
                push_interrupt(process, interrupts, IRQ_DMA);
            }
            // memory fill: two buffers (start, value, end) and their control halfwords
            2 => {
                let control = word(7);
                for (buffer, start, value, end, ctl) in [
                    (0, word(1), word(2), word(3), control & 0xFFFF),
                    (1, word(4), word(5), word(6), control >> 16),
                ] {
                    if start == 0 {
                        continue;
                    }
                    fill(process, start, end, value, (ctl >> 8) & 3);
                    host.say(format!("gsp: GX memory fill {buffer}: {start:#x}..{end:#x} = {value:#x}"));
                    push_interrupt(process, interrupts, if buffer == 0 { IRQ_PSC0 } else { IRQ_PSC1 });
                }
            }
            // flush cache regions: coherent host, nothing to do
            5 => {}
            id => {
                let name = match id {
                    1 => "process command list",
                    3 => "display transfer",
                    4 => "texture copy",
                    _ => "an unknown GX command",
                };
                host.gap(format!("gsp: GX {name} ({id}) not executed (PICA is 3DS-W4)"));
            }
        }
        index = (index + 1) % QUEUE_COMMANDS;
    }
    memory.write8(queue, index as u8);
    memory.write8(queue + 1, 0);
    let event = host.gsp_event.load(std::sync::atomic::Ordering::Acquire);
    if total > 0 && event != 0 {
        process.signal_event(event);
    }
}

/// a GX memory fill: `width` 0 = 16-bit, 1 = 24-bit, 2/3 = 32-bit pattern (3dbrew GX MemoryFill)
pub fn fill(process: &Process3ds, start: u32, end: u32, value: u32, width: u32) {
    let pattern: Vec<u8> = match width {
        0 => (value as u16).to_le_bytes().to_vec(),
        1 => value.to_le_bytes()[..3].to_vec(),
        _ => value.to_le_bytes().to_vec(),
    };
    let size = end.saturating_sub(start).min(0x0100_0000) as usize;
    let bytes: Vec<u8> = pattern.iter().copied().cycle().take(size).collect();
    process.memory.write_bytes(start, &bytes);
}

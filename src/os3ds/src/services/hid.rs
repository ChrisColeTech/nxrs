//! hid: the buttons/touch/circle-pad feed. `GetIPCHandles` (0x000A0000) returns a real shared-memory
//! block plus five real kernel events; the scripted input thread writes the HID shared-memory PAD,
//! circle-pad and touch entries (3dbrew `HID Shared Memory` rev 24161) and signals the pad event.

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID, R_INVALID_CMD, SharedKind};
use runtime3ds::Process3ds;

pub const GET_IPC_HANDLES: u32 = 0x000A;
pub const ENABLE_ACCELEROMETER: u32 = 0x0011;
pub const DISABLE_ACCELEROMETER: u32 = 0x0012;
pub const ENABLE_GYROSCOPE: u32 = 0x0013;
pub const DISABLE_GYROSCOPE: u32 = 0x0014;
pub const GET_GYROSCOPE_COEFFICIENT: u32 = 0x0015;
pub const GET_GYROSCOPE_CALIBRATE: u32 = 0x0016;
pub const GET_SOUND_VOLUME: u32 = 0x0017;
pub const CALIBRATE_TOUCH: u32 = 0x0001;

/// the block HID serves (3dbrew's 0x2B0 region, rounded to a page)
pub const SHARED_SIZE: u32 = 0x1000;

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        GET_IPC_HANDLES => {
            let Some(shared) = host.register_shared(process, SharedKind::Hid, SHARED_SIZE) else {
                return Reply::err(R_INVALID);
            };
            // five update events (3dbrew: sharedmem pad/touch x2, accelerometer, gyroscope, debugpad)
            let events: Vec<u32> = (0..5).map(|_| process.create_event(1)).collect();
            {
                let mut state = host.state.lock().unwrap();
                state.hid_events = events.clone();
            }
            host.hid_event.store(events[0], std::sync::atomic::Ordering::Release);
            host.note_serve();
            host.say(format!(
                "hid: GetIPCHandles -> shared {shared:#x}, events {events:08x?}"
            ));
            // response: result, descriptor 0x14000000 (6 handles), shared handle + 5 events
            let mut words = vec![R_OK, 0x1400_0000, shared];
            words.extend_from_slice(&events);
            Reply::new(words, 7)
        }
        ENABLE_ACCELEROMETER | DISABLE_ACCELEROMETER | ENABLE_GYROSCOPE | DISABLE_GYROSCOPE => {
            Reply::ok()
        }
        GET_GYROSCOPE_COEFFICIENT => Reply::new(vec![R_OK, 0], 0),
        GET_GYROSCOPE_CALIBRATE => Reply::new(vec![R_OK, 0, 0], 0),
        GET_SOUND_VOLUME => Reply::new(vec![R_OK, 0x40], 0),
        CALIBRATE_TOUCH => Reply::ok(),
        other => {
            host.gap(format!("hid: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle::{parse_input, Host};
    use runtime3ds::context;
    use runtime3ds::dispatch::Library;
    use runtime3ds::mem::{Kind, Memory, Perm, State};
    use runtime3ds::BootOptions;
    use std::sync::Arc;

    fn process() -> (Arc<Memory>, Box<Process3ds>) {
        let memory = Arc::new(Memory::new().unwrap());
        memory
            .map(0x0800_0000, 0x0010_0000, Perm::Rw, State::Private, Kind::Heap)
            .unwrap();
        let library = unsafe {
            Library::linked(context::ABI, [].as_ptr(), 0, std::ptr::null(), 0, std::ptr::null(), 0)
        };
        (memory.clone(), Box::new(Process3ds::new(library, memory, BootOptions::default())))
    }

    #[test]
    fn get_ipc_handles_returns_the_shared_block_and_five_events() {
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let (_memory, process) = process();
        let request = Request { cmd: GET_IPC_HANDLES, normal: 0, translate: 0, params: vec![], translated: vec![] };
        let reply = command(&host, &process, 0, &request);
        assert_eq!(reply.words[0], R_OK);
        assert_eq!(reply.translate, 7);
        assert_eq!(reply.words[1], 0x1400_0000);
        assert_ne!(reply.words[2], 0, "shared block handle");
        for handle in &reply.words[3..9] {
            assert_ne!(*handle, 0, "an event handle");
        }
        assert_eq!(host.state.lock().unwrap().hid_events.len(), 5);
    }

    #[test]
    fn the_scripted_pad_state_lands_in_the_shared_memory() {
        let script = parse_input("0 neutral\n10 a+left\n").unwrap();
        let host = Host::new(std::env::temp_dir(), script);
        let (memory, process) = process();
        // create and map a HID block at a free address
        let handle = host.register_shared(&process, SharedKind::Hid, SHARED_SIZE).unwrap();
        assert!(host.map_block(&process, handle, 0x1F00_0000));
        host.input.lock().unwrap().advance(50);
        host.write_pad(&process);
        // PAD bits: A = 0, Left = 5 -> 0x21
        assert_eq!(memory.read32(0x1F00_001C), 0x21);
        assert_eq!(memory.read32(0x1F00_0028), 0x21);
    }
}

//! hid: the buttons/touch/circle-pad feed (3dbrew `HID Services`). `GetIPCHandles` (0x000A0000)
//! returns a real shared-memory block plus five real one-shot kernel events (PAD0, PAD1,
//! accelerometer, gyroscope, debug pad). The frame clock (`Host::frame`) publishes the scripted
//! pad state as the next PAD/touch ring entry (3dbrew `HID Shared Memory`) and signals PAD0.
//! The accelerometer and gyroscope publish no samples headless (named in the ledger).

use crate::hle::{Host, Reply, Request, SharedKind, R_INVALID, R_INVALID_CMD, R_OK};
use runtime3ds::Process3ds;

pub const GET_IPC_HANDLES: u32 = 0x000A;
pub const ENABLE_ACCELEROMETER: u32 = 0x0011;
pub const DISABLE_ACCELEROMETER: u32 = 0x0012;
pub const ENABLE_GYROSCOPE: u32 = 0x0013;
pub const DISABLE_GYROSCOPE: u32 = 0x0014;
pub const GET_GYROSCOPE_COEFFICIENT: u32 = 0x0015;
pub const GET_SOUND_VOLUME: u32 = 0x0017;

/// the block HID serves (3dbrew's 0x2B0 region, rounded to a page)
pub const SHARED_SIZE: u32 = 0x1000;
/// the gyroscope's raw-to-degrees-per-second coefficient (3dbrew HID:GetGyroscopeLowRawToDpsCoefficient)
pub const GYRO_COEFFICIENT: f32 = 14.375;
/// the volume slider, 0-63: full
pub const SOUND_VOLUME: u32 = 0x3F;

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        GET_IPC_HANDLES => {
            let existing = host.state.lock().unwrap().hid_shared;
            let shared = match existing {
                Some(shared) => shared,
                None => match host.register_shared(process, SharedKind::Hid, SHARED_SIZE) {
                    Some(shared) => shared,
                    None => return Reply::err(R_INVALID),
                },
            };
            let events = {
                let known = host.state.lock().unwrap().hid_events.clone();
                if known.is_empty() {
                    let made: Vec<u32> = (0..5).map(|_| process.create_event(0)).collect();
                    host.state.lock().unwrap().hid_events = made.clone();
                    made
                } else {
                    known
                }
            };
            host.hid_event.store(events[0], std::sync::atomic::Ordering::Release);
            host.note_serve();
            host.say(format!("hid: GetIPCHandles -> shared {shared:#x}, events {events:08x?}"));
            // response: result, copy-handles descriptor ((6 - 1) << 26), shared block + 5 events
            let mut words = vec![R_OK, 0x1400_0000, shared];
            words.extend_from_slice(&events);
            Reply::new(words, 7)
        }
        ENABLE_ACCELEROMETER | ENABLE_GYROSCOPE => {
            host.gap(format!(
                "hid: {} enabled, but no motion samples are published headless",
                if request.cmd == ENABLE_ACCELEROMETER { "accelerometer" } else { "gyroscope" }
            ));
            Reply::ok()
        }
        DISABLE_ACCELEROMETER | DISABLE_GYROSCOPE => Reply::ok(),
        GET_GYROSCOPE_COEFFICIENT => Reply::new(vec![R_OK, GYRO_COEFFICIENT.to_bits()], 0),
        GET_SOUND_VOLUME => Reply::new(vec![R_OK, SOUND_VOLUME], 0),
        other => {
            host.gap(format!("hid: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

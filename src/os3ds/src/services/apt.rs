//! apt: the applet lifecycle. The state machine answers the SDK's init and menu-state calls from
//! the 3dbrew `NS and APT Services` table; the two Initialize events are real kernel events so a
//! `svcWaitSynchronization` on them works.

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const GET_LOCK_HANDLE: u32 = 0x0001;
pub const INITIALIZE: u32 = 0x0002;
pub const ENABLE: u32 = 0x0003;
pub const FINALIZE: u32 = 0x0004;
pub const GET_APPLET_MAN_INFO: u32 = 0x0005;
pub const GET_APPLET_INFO: u32 = 0x0006;
pub const GET_LAST_SIGNALED_APPLET_ID: u32 = 0x0007;
pub const COUNT_REGISTERED_APPLET: u32 = 0x0008;
pub const IS_REGISTERED: u32 = 0x0009;
pub const GET_ATTRIBUTE: u32 = 0x000A;
pub const INQUIRE_NOTIFICATION: u32 = 0x000B;
pub const RECEIVE_PARAMETER: u32 = 0x000D;
pub const GLANCE_PARAMETER: u32 = 0x000E;
pub const CANCEL_PARAMETER: u32 = 0x000F;
pub const GET_PREPARATION_STATE: u32 = 0x0013;
pub const SET_PREPARATION_STATE: u32 = 0x0014;
pub const NOTIFY_TO_WAIT: u32 = 0x0043;
pub const GET_SHARED_FONT: u32 = 0x0044;
pub const GET_PROGRAM_ID: u32 = 0x0058;
pub const APPLET_UTILITY: u32 = 0x004B;
pub const GET_PROGRAM_INFO: u32 = 0x0048;
pub const SET_FATAL_ERRDISP_MODE: u32 = 0x004C;
pub const GET_APPLET_PROGRAM_INFO: u32 = 0x004D;
pub const HARDWARE_RESET_ASYNC: u32 = 0x004E;
pub const SET_APPLICATION_CPU_TIME_LIMIT: u32 = 0x004F;
pub const GET_APPLICATION_CPU_TIME_LIMIT: u32 = 0x0050;
pub const GET_STARTUP_ARGUMENT: u32 = 0x0051;
pub const SET_SCREEN_CAPTURE_POST_PERMISSION: u32 = 0x0055;
pub const GET_SCREEN_CAPTURE_POST_PERMISSION: u32 = 0x0056;
pub const GET_CAPTURE_INFO: u32 = 0x004A;
pub const GET_TARGET_PLATFORM: u32 = 0x0101;
pub const CHECK_NEW_3DS: u32 = 0x0102;
pub const GET_APPLICATION_RUNNING_MODE: u32 = 0x0103;
pub const IS_STANDARD_MEMORY_LAYOUT: u32 = 0x0104;

/// the application applet id and POS_APP
const APP_ID: u32 = 0x300;

/// APT signal type Wakeup (3dbrew `NS and APT Services` §Signal)
pub const SIGNAL_WAKEUP: u32 = 1;

pub fn command(host: &Host, process: &Process3ds, tls: u32, request: &Request) -> Reply {
    match request.cmd {
        GET_LOCK_HANDLE => {
            // the APT lock is a mutex the service hands out already held; the app releases it
            let lock = host.state.lock().unwrap().fresh_handle();
            host.set_apt_lock(lock);
            host.say(format!("apt: GetLockHandle -> {lock:#x}"));
            // named once: the lock is held service-side because runtime3ds's WaitSynchronization
            // does not take ownership of a kernel mutex (so its ReleaseMutex would fail)
            host.gap("apt: the APT lock is a service-side lock (runtime3ds waits do not acquire kernel mutexes)");
            Reply::new(vec![R_OK, 0, 0, 0, lock], 2)
        }
        INITIALIZE => {
            // response: result, translation descriptor 0x04000000 + the notification and
            // parameter events (one-shot). An application starts with a Wakeup parameter
            // pending, so the parameter event starts signaled.
            let notification = process.create_event(0);
            let parameter = process.create_event(0);
            process.signal_event(parameter);
            {
                let mut state = host.state.lock().unwrap();
                state.apt.initialized = true;
                state.apt.applet_id = request.word(0).max(APP_ID);
                state.apt.applet_pos = request.word(1) & 0b111;
                state.apt.parameter_pending = true;
                state.apt_parameter_event = Some(parameter);
            }
            host.say(format!(
                "apt: Initialize appID {:#x}, attr {:#x} (notification {notification:#x}, parameter {parameter:#x})",
                request.word(0),
                request.word(1)
            ));
            Reply::new(vec![R_OK, 0x0400_0000, notification, parameter], 3)
        }
        ENABLE => {
            host.state.lock().unwrap().apt.enabled = true;
            Reply::ok()
        }
        FINALIZE => {
            let mut state = host.state.lock().unwrap();
            state.apt.enabled = false;
            state.apt.initialized = false;
            Reply::ok()
        }
        GET_APPLET_MAN_INFO => {
            let pos = request.word(0);
            let state = host.state.lock().unwrap();
            let app = state.apt.applet_id;
            Reply::new(vec![R_OK, pos, app, 0x101, app], 0)
        }
        GET_APPLET_INFO => {
            let state = host.state.lock().unwrap();
            let app = request.word(0);
            // the system applets the SDK checks for (e.g. the home menu) report a registered,
            // loaded title; the application itself reports this title
            let tid = if app == APP_ID || app == state.apt.applet_id {
                0x0004_0000_0006_2300u64
            } else if app >= 0x100 {
                0x0004_0030_0000_0101u64
            } else {
                0
            };
            Reply::new(
                vec![R_OK, tid as u32, (tid >> 32) as u32, 0, 1, 1, state.apt.applet_pos],
                0,
            )
        }
        GET_LAST_SIGNALED_APPLET_ID => {
            let id = host.state.lock().unwrap().apt.last_signaled;
            Reply::new(vec![R_OK, id], 0)
        }
        COUNT_REGISTERED_APPLET => Reply::new(vec![R_OK, 1], 0),
        IS_REGISTERED => Reply::new(vec![R_OK, (request.word(0) == APP_ID || request.word(0) >= 0x100) as u32], 0),
        GET_ATTRIBUTE => Reply::new(vec![R_OK, 0], 0),
        INQUIRE_NOTIFICATION => Reply::new(vec![R_OK, 0], 0),
        RECEIVE_PARAMETER | GLANCE_PARAMETER => {
            // the initial wakeup parameter is delivered once; Glance does not clear it, Receive
            // does (3dbrew APT:ReceiveParameter's pending flag)
            let pending = host.state.lock().unwrap().apt.parameter_pending;
            if !pending {
                // 3dbrew's "no parameter pending" result
                return Reply::err(0xC8A0_CFFA);
            }
            if request.cmd == RECEIVE_PARAMETER {
                // delivered: the parameter event's one-shot signal is consumed with it
                let event = {
                    let mut state = host.state.lock().unwrap();
                    state.apt.parameter_pending = false;
                    state.apt_parameter_event
                };
                if let Some(event) = event {
                    process.clear_event(event);
                }
            }
            host.say(format!("apt: {} -> Wakeup", if request.cmd == RECEIVE_PARAMETER { "ReceiveParameter" } else { "GlanceParameter" }));
            // response (3dbrew 0x000D0104): result, sender AppID, signal type (1 = Wakeup),
            // actual parameter size; a move-handle descriptor + handle (none), and the static
            // buffer descriptor + the receiving thread's static buffer 0 (TLS+0x184; no payload)
            let buffer = process.memory.read32(tls.wrapping_add(0x184));
            Reply::new(vec![R_OK, 0, SIGNAL_WAKEUP, 0, 0x10, 0, 2, buffer], 4)
        }
        CANCEL_PARAMETER => Reply::ok(),
        GET_PREPARATION_STATE => {
            let preparation = host.state.lock().unwrap().apt.preparation;
            Reply::new(vec![R_OK, preparation], 0)
        }
        SET_PREPARATION_STATE => {
            host.state.lock().unwrap().apt.preparation = request.word(0);
            Reply::ok()
        }
        NOTIFY_TO_WAIT => Reply::ok(),
        GET_SHARED_FONT => {
            // a zero page for now: W4 owns the font. The map address is ours to pick, outside the
            // exe/heap/stack/LINEAR windows; the caller maps the returned handle there.
            let handle = host.register_shared(process, crate::hle::SharedKind::Other, 0x1000);
            match handle {
                Some(handle) => {
                    host.gap("apt:GetSharedFont returns a zero page (the real shared font is 3DS-W4)");
                    Reply::new(vec![R_OK, 0x1E00_0000, 0, handle], 2)
                }
                None => Reply::err(crate::hle::R_INVALID),
            }
        }
        GET_PROGRAM_ID => Reply::new(vec![R_OK, 0x0006_2300, 0x0004_0000], 0),
        APPLET_UTILITY => {
            // word 1 = utility id. There is no applet manager headless (no home menu, no other
            // applet), so the utility is named in the ledger; its out-value is 0 (none).
            host.say(format!("apt: AppletUtility id {:#x}", request.word(0)));
            host.gap(format!(
                "apt: AppletUtility {:#x} has no applet manager behind it (answered 0)",
                request.word(0)
            ));
            Reply::new(vec![R_OK, 0], 0)
        }
        GET_PROGRAM_INFO | GET_APPLET_PROGRAM_INFO => {
            host.gap(format!("apt: command {:#06x} (program info) not implemented", request.cmd));
            Reply::err(R_INVALID_CMD)
        }
        SET_FATAL_ERRDISP_MODE => Reply::ok(),
        HARDWARE_RESET_ASYNC | GET_CAPTURE_INFO => {
            host.gap(format!("apt: command {:#06x} refused headless", request.cmd));
            Reply::err(R_INVALID_CMD)
        }
        SET_APPLICATION_CPU_TIME_LIMIT => Reply::ok(),
        GET_APPLICATION_CPU_TIME_LIMIT => Reply::new(vec![R_OK, 0], 0),
        GET_STARTUP_ARGUMENT => Reply::new(vec![R_OK, 0, 0], 0),
        SET_SCREEN_CAPTURE_POST_PERMISSION => Reply::ok(),
        GET_SCREEN_CAPTURE_POST_PERMISSION => Reply::new(vec![R_OK, 0], 0),
        GET_TARGET_PLATFORM => Reply::new(vec![R_OK, 0], 0),
        CHECK_NEW_3DS => Reply::new(vec![R_OK, 0], 0),
        GET_APPLICATION_RUNNING_MODE => Reply::new(vec![R_OK, 0], 0),
        IS_STANDARD_MEMORY_LAYOUT => Reply::new(vec![R_OK, 1], 0),
        other => {
            host.gap(format!("apt: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle::Host;
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

    fn request(cmd: u32, params: Vec<u32>) -> Request {
        Request { cmd, normal: params.len(), translate: 0, params, translated: Vec::new() }
    }

    #[test]
    fn initialize_then_enable_sets_the_state() {
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let (_memory, process) = process();
        let reply = command(&host, &process, 0, &request(INITIALIZE, vec![0x300, 0]));
        assert_eq!(reply.words[0], R_OK);
        assert_eq!(reply.translate, 3);
        assert_eq!(reply.words[1], 0x0400_0000);
        assert_ne!(reply.words[2], 0, "notification event");
        assert_ne!(reply.words[3], 0, "resume event");
        assert!(host.state.lock().unwrap().apt.initialized);
        command(&host, &process, 0, &request(ENABLE, vec![]));
        assert!(host.state.lock().unwrap().apt.enabled);
    }

    #[test]
    fn get_applet_man_info_answers_the_menu_state() {
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let (_memory, process) = process();
        command(&host, &process, 0, &request(INITIALIZE, vec![0x300, 0]));
        let reply = command(&host, &process, 0, &request(GET_APPLET_MAN_INFO, vec![0]));
        assert_eq!(reply.words, vec![R_OK, 0, 0x300, 0x101, 0x300]);
    }

    #[test]
    fn an_unknown_apt_command_is_a_gap() {
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let (_memory, process) = process();
        let reply = command(&host, &process, 0, &request(0x00FF_0000, vec![]));
        assert_eq!(reply.words[0], R_INVALID_CMD);
        assert_eq!(host.gaps()[0].1, 1);
    }
}

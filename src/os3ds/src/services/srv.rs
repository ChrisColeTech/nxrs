//! srv: the service manager. `ConnectToPort("srv:")` hands out the srv session; SRV
//! `GetServiceHandle` (0x00050100) creates a session per name; `err:f` is a port of its own.
//! 3dbrew `Services` rev 24351, `SRV:GetServiceHandle` rev 24639.

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID_CMD};
use runtime3ds::Process3ds;

/// SRV:GetServiceHandle
pub const GET_SERVICE_HANDLE: u32 = 0x0005;
/// SRV:EnableNotification
pub const ENABLE_NOTIFICATION: u32 = 0x0002;
/// SRV:RegisterClient
pub const REGISTER_CLIENT: u32 = 0x0001;
/// SRV:IsServiceRegistered
pub const IS_SERVICE_REGISTERED: u32 = 0x000E;

/// the 8-byte UTF-8 name in request words 1-2 (the caller's `params[0..2]`), NUL-terminated
pub fn name_of(request: &Request) -> String {
    let mut bytes = Vec::new();
    for word in request.params.iter().take(2) {
        for byte in word.to_le_bytes() {
            if byte == 0 {
                return String::from_utf8_lossy(&bytes).into_owned();
            }
            bytes.push(byte);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn command(
    host: &Host,
    process: &Process3ds,
    _tls: u32,
    _session: u32,
    request: &Request,
) -> Reply {
    match request.cmd {
        ENABLE_NOTIFICATION => {
            // 3dbrew SRV:EnableNotification, response 0x00020042: result, copy-handle
            // descriptor 0, the notification semaphore. Headless nothing is ever published, so
            // the semaphore stays at zero; a wait on it parks like on hardware.
            let semaphore = host.srv_semaphore(process);
            host.say(format!("srv: EnableNotification -> semaphore {semaphore:#x}"));
            Reply::new(vec![R_OK, 0, semaphore], 2)
        }
        // 3dbrew SRV:RegisterClient, response 0x00010040: the result only
        REGISTER_CLIENT => Reply::ok(),
        GET_SERVICE_HANDLE => {
            let name = name_of(request);
            match host.connect(&name) {
                Some(handle) => Reply::new(vec![R_OK, 0, handle], 2),
                None => {
                    host.gap(format!("srv:GetServiceHandle: {name:?} is not registered"));
                    Reply::err(crate::hle::R_NOT_FOUND)
                }
            }
        }
        IS_SERVICE_REGISTERED => {
            let name = name_of(request);
            let known = host.service_known(&name);
            Reply::new(vec![R_OK, known as u32], 0)
        }
        other => {
            host.gap(format!("srv: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

/// err:f Throw (3dbrew `ERR:Throw`, 0x00010800): the 0x80-byte fatal-error record
pub const ERRF_THROW: u32 = 0x0001;
/// err:f SetUserString (0x00020042)
pub const ERRF_SET_USER_STRING: u32 = 0x0002;

/// err:f: ErrDisp. A Throw is the title reporting a fatal error: ErrDisp shows it and the
/// title stops. Headless the record is put in the gap ledger (type, result, pc) and the reply
/// returns, so the trace shows exactly what the title considered fatal.
pub fn errf_command(
    host: &Host,
    _process: &Process3ds,
    _tls: u32,
    request: &Request,
) -> Reply {
    match request.cmd {
        ERRF_THROW => {
            // record word 0 low byte = type, word 1 = result, word 3 = pc (3dbrew ERR:Throw)
            let kind = request.word(0) & 0xFF;
            let result = request.word(1);
            let pc = request.word(3);
            host.gap(format!(
                "err:f Throw: type {kind}, result {result:#010x}, pc {pc:#x} (the title reported a fatal error)"
            ));
            Reply::ok()
        }
        ERRF_SET_USER_STRING => {
            host.say("err:f: SetUserString".to_owned());
            Reply::ok()
        }
        other => {
            host.gap(format!("err:f: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_reads_from_two_words() {
        let request = Request {
            cmd: GET_SERVICE_HANDLE,
            normal: 4,
            translate: 2,
            params: vec![u32::from_le_bytes(*b"fs:U"), u32::from_le_bytes(*b"SER\0"), 8, 0],
            translated: vec![0, 0],
        };
        assert_eq!(name_of(&request), "fs:USER");
    }
}

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
/// SRV:UnregisterService
pub const UNREGISTER_SERVICE: u32 = 0x0004;

/// the 8-byte UTF-8 name in request words 1-2 (the caller's `params[0..2]`), NUL-terminated
fn name_of(request: &Request) -> String {
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
    _process: &Process3ds,
    _tls: u32,
    _session: u32,
    request: &Request,
) -> Reply {
    match request.cmd {
        ENABLE_NOTIFICATION => Reply::ok(),
        REGISTER_CLIENT => {
            // response: result, translate descriptor 0 + a notification handle (the SDK stores it)
            Reply::new(vec![R_OK, 0, 0], 2)
        }
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
        UNREGISTER_SERVICE => Reply::ok(),
        other => {
            host.gap(format!("srv: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

/// err:f: ErrDisp accepts a Throw and returns; SetUserString/other handshakes are answered so a
/// title that reports a handled error does not wedge
pub fn errf_command(
    host: &Host,
    _process: &Process3ds,
    _tls: u32,
    request: &Request,
) -> Reply {
    match request.cmd {
        // Throw / SetUserString / ... (the command numbers behind 0x00010040 etc.)
        0x0001 | 0x0002 | 0x0003 | 0x0004 => {
            host.say(format!(
                "err:f command {:#06x} params {:08x?}",
                request.cmd, request.params
            ));
            Reply::ok()
        }
        other => {
            host.gap(format!("err:f: command {other:#010x} not implemented"));
            Reply::ok()
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

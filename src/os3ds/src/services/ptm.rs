//! ptm: battery, shell and play-time handshakes (3dbrew `PTM Services`). The values are plausible
//! fixed ones: a desktop host is charged and full, the shell is open, and the step counters are
//! zero (no pedometer).

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const GET_ADAPTER_STATE: u32 = 0x0001;
pub const GET_BATTERY_LEVEL: u32 = 0x0002;
pub const GET_BATTERY_CHARGE_STATE: u32 = 0x0003;
pub const GET_SHELL_STATE: u32 = 0x0004;
pub const IS_CHARGING: u32 = 0x0005;
pub const GET_PEDOMETER_STATE: u32 = 0x0006;
pub const GET_STEP_HISTORY: u32 = 0x000B;
pub const GET_TOTAL_STEP_COUNT: u32 = 0x000C;

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        GET_ADAPTER_STATE => Reply::new(vec![R_OK, 0], 0),
        GET_BATTERY_LEVEL => Reply::new(vec![R_OK, 4], 0),
        GET_BATTERY_CHARGE_STATE => Reply::new(vec![R_OK, 1], 0),
        GET_SHELL_STATE => Reply::new(vec![R_OK, 0], 0),
        IS_CHARGING => Reply::new(vec![R_OK, 1], 0),
        GET_PEDOMETER_STATE => Reply::new(vec![R_OK, 0], 0),
        GET_STEP_HISTORY => {
            // words: hours, DateTime (2), translate = buffer size + pointer
            let hours = request.word(0).min(0x1000) as usize;
            let buffer = request.translated.get(1).copied().unwrap_or(0);
            if buffer != 0 {
                process.memory.write_bytes(buffer, &vec![0u8; hours * 2]);
            }
            Reply::ok()
        }
        GET_TOTAL_STEP_COUNT => Reply::new(vec![R_OK, 0], 0),
        other => {
            host.gap(format!("ptm: command {other:#010x} not implemented"));
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

    #[test]
    fn the_battery_is_full_and_charging() {
        let (_memory, process) = process();
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let request = Request { cmd: GET_BATTERY_LEVEL, normal: 0, translate: 0, params: vec![], translated: vec![] };
        assert_eq!(command(&host, &process, 0, &request).words, vec![R_OK, 4]);
    }

    #[test]
    fn the_step_queries_answer_zero() {
        let (_memory, process) = process();
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let request = Request { cmd: GET_TOTAL_STEP_COUNT, normal: 0, translate: 0, params: vec![], translated: vec![] };
        assert_eq!(command(&host, &process, 0, &request).words, vec![R_OK, 0]);
    }
}

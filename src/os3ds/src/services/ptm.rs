//! ptm: battery, shell and pedometer queries (3dbrew `PTM Services`, ptm:u command table). A
//! desktop host is on the adapter, charging, full and open; there is no pedometer, so the step
//! counters are zero. The RTC alarm commands are refused with a gap line.

use crate::hle::{Host, Reply, Request, R_INVALID_CMD, R_OK};
use runtime3ds::Process3ds;

pub const REGISTER_ALARM_CLIENT: u32 = 0x0001;
pub const SET_RTC_ALARM: u32 = 0x0002;
pub const GET_RTC_ALARM: u32 = 0x0003;
pub const CANCEL_RTC_ALARM: u32 = 0x0004;
pub const GET_ADAPTER_STATE: u32 = 0x0005;
pub const GET_SHELL_STATE: u32 = 0x0006;
pub const GET_BATTERY_LEVEL: u32 = 0x0007;
pub const GET_BATTERY_CHARGE_STATE: u32 = 0x0008;
pub const GET_PEDOMETER_STATE: u32 = 0x0009;
pub const GET_STEP_HISTORY: u32 = 0x000B;
pub const GET_TOTAL_STEP_COUNT: u32 = 0x000C;

/// battery level 0-5 (5 = full)
pub const BATTERY_FULL: u32 = 5;

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        // 1 = the adapter is connected
        GET_ADAPTER_STATE => Reply::new(vec![R_OK, 1], 0),
        // 1 = the shell is open
        GET_SHELL_STATE => Reply::new(vec![R_OK, 1], 0),
        GET_BATTERY_LEVEL => Reply::new(vec![R_OK, BATTERY_FULL], 0),
        // 1 = charging
        GET_BATTERY_CHARGE_STATE => Reply::new(vec![R_OK, 1], 0),
        // 0 = not counting
        GET_PEDOMETER_STATE => Reply::new(vec![R_OK, 0], 0),
        GET_STEP_HISTORY => {
            // 3dbrew PTM:GetStepHistory (0x000B00C2): word 1 = hours, words 2-3 = start time;
            // translate: a mapped-buffer descriptor + the u16-per-hour output buffer
            let hours = request.word(0).min(0x1000) as usize;
            let buffer = request.translated.get(1).copied().unwrap_or(0);
            if buffer != 0 {
                process.memory.write_bytes(buffer, &vec![0u8; hours * 2]);
            }
            Reply::ok()
        }
        GET_TOTAL_STEP_COUNT => Reply::new(vec![R_OK, 0], 0),
        REGISTER_ALARM_CLIENT | SET_RTC_ALARM | GET_RTC_ALARM | CANCEL_RTC_ALARM => {
            host.gap(format!("ptm: RTC alarm command {:#06x} refused", request.cmd));
            Reply::err(R_INVALID_CMD)
        }
        other => {
            host.gap(format!("ptm: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

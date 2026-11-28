//! misc: the small services a title may only open (mic, cam, boss, news, ndm, ir). ndm:u's
//! daemon controls answer: there are no network daemons headless, so suspending, resuming or
//! overriding them has nothing to change (3dbrew `NDM Services`). Every other command is refused
//! with a gap line naming it (plan §4's "defer/refuse" rows).

use crate::hle::{Host, Reply, Request, R_INVALID_CMD, R_OK};
use runtime3ds::Process3ds;

/// ndm:u daemon controls: Enter/LeaveExclusiveState 1/2, Suspend/ResumeDaemons 6/7,
/// Suspend/ResumeScheduler 8/9, OverrideDefaultDaemons 0x14, ResetDefaultDaemons 0x15
const NDM_CONTROLS: [u32; 8] = [0x0001, 0x0002, 0x0006, 0x0007, 0x0008, 0x0009, 0x0014, 0x0015];
/// ndm:u QueryExclusiveMode
const NDM_QUERY_EXCLUSIVE_MODE: u32 = 0x0003;

pub fn command(
    host: &Host,
    _process: &Process3ds,
    _tls: u32,
    name: &'static str,
    request: &Request,
) -> Reply {
    if name == "ndm:u" {
        if NDM_CONTROLS.contains(&request.cmd) {
            host.say(format!("ndm:u: daemon control {:#06x} (no daemons headless)", request.cmd));
            return Reply::ok();
        }
        if request.cmd == NDM_QUERY_EXCLUSIVE_MODE {
            return Reply::new(vec![R_OK, 0], 0);
        }
    }
    host.gap(format!("{name}: command {:#06x} refused (this service is deferred)", request.cmd));
    Reply::err(R_INVALID_CMD)
}
